//! What indexing and search need from the embedding model, and the client for
//! llama-server's OpenAI-style `/v1/embeddings` endpoint.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

/// Stored with the index so vectors from a different model are never mixed.
pub const MODEL_ID: &str = "embeddinggemma-2-Q8_0";
pub const DIM: usize = 768;

pub enum Input {
    Text(String),
    /// Base64-encoded JPEG.
    Image(String),
}

/// Turns inputs into vectors. The app uses llama-server; tests use a fake.
pub trait Embedder {
    /// Returns one L2-normalised vector per input, in input order.
    fn embed(&self, inputs: &[Input]) -> Result<Vec<Vec<f32>>>;
}

/// Talks to a running llama-server over HTTP.
pub struct HttpEmbedder {
    url: String,
}

impl HttpEmbedder {
    /// `base_url` is the server root, such as `http://127.0.0.1:8080`.
    pub fn new(base_url: &str) -> HttpEmbedder {
        HttpEmbedder { url: format!("{}/v1/embeddings", base_url.trim_end_matches('/')) }
    }
}

impl Embedder for HttpEmbedder {
    fn embed(&self, inputs: &[Input]) -> Result<Vec<Vec<f32>>> {
        let resp: Value = ureq::post(&self.url)
            .timeout(Duration::from_secs(600))
            .send_json(request_body(inputs))
            .map_err(|e| match e {
                ureq::Error::Status(code, r) => {
                    anyhow!("llama-server {code}: {}", r.into_string().unwrap_or_default())
                }
                other => anyhow!(other),
            })?
            .into_json()?;
        parse_response(&resp, inputs.len())
    }
}

pub fn request_body(inputs: &[Input]) -> Value {
    let items: Vec<Value> = inputs
        .iter()
        .map(|i| match i {
            Input::Text(t) => json!(t),
            Input::Image(b64) => json!({ "content": [{
                "type": "image_url",
                "image_url": { "url": format!("data:image/jpeg;base64,{b64}") }
            }]}),
        })
        .collect();
    json!({ "input": items, "encoding_format": "float" })
}

/// Reads the vectors out of an embeddings response for `count` inputs, normalised and
/// placed by their `index`.
pub fn parse_response(resp: &Value, count: usize) -> Result<Vec<Vec<f32>>> {
    let data = resp["data"].as_array().context("embeddings response has no data")?;
    let mut out = vec![Vec::new(); count];
    for d in data {
        let idx = d["index"].as_u64().context("embedding without index")? as usize;
        let mut v: Vec<f32> = d["embedding"]
            .as_array()
            .context("embedding is not an array")?
            .iter()
            .map(|x| x.as_f64().unwrap_or(0.0) as f32)
            .collect();
        if v.len() != DIM {
            bail!("expected {DIM}-d embedding, got {}", v.len());
        }
        normalize(&mut v);
        *out.get_mut(idx).context("embedding index out of range")? = v;
    }
    if out.iter().any(|v| v.is_empty()) {
        bail!("llama-server returned fewer embeddings than inputs");
    }
    Ok(out)
}

pub fn normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockServer;

    fn vector(hot: usize, scale: f32) -> Vec<f32> {
        let mut v = vec![0.0; DIM];
        v[hot] = scale;
        v
    }

    #[test]
    fn request_body_wraps_images_as_data_urls() {
        let body = request_body(&[Input::Text("hello".into()), Input::Image("QUJD".into())]);
        assert_eq!(body["input"][0], "hello");
        assert_eq!(body["input"][1]["content"][0]["image_url"]["url"], "data:image/jpeg;base64,QUJD");
        assert_eq!(body["encoding_format"], "float");
    }

    #[test]
    fn parse_orders_by_index_and_normalises() {
        let resp = json!({ "data": [
            { "index": 1, "embedding": vector(1, 3.0) },
            { "index": 0, "embedding": vector(0, 2.0) },
        ]});
        let out = parse_response(&resp, 2).unwrap();
        assert_eq!(out[0], vector(0, 1.0));
        assert_eq!(out[1], vector(1, 1.0));
    }

    #[test]
    fn parse_rejects_bad_responses() {
        let short = json!({ "data": [{ "index": 0, "embedding": [1.0, 2.0] }] });
        assert!(parse_response(&short, 1).unwrap_err().to_string().contains("768-d"));

        let missing = json!({ "data": [{ "index": 0, "embedding": vector(0, 1.0) }] });
        assert!(parse_response(&missing, 2).unwrap_err().to_string().contains("fewer"));

        let out_of_range = json!({ "data": [{ "index": 5, "embedding": vector(0, 1.0) }] });
        assert!(parse_response(&out_of_range, 1).is_err());

        assert!(parse_response(&json!({ "error": "boom" }), 1).is_err());
    }

    #[test]
    fn http_embedder_posts_to_the_server() {
        let reply = json!({ "data": [{ "index": 0, "embedding": vector(7, 5.0) }] });
        let server = MockServer::start(200, reply.to_string());
        let out = HttpEmbedder::new(&server.url).embed(&[Input::Text("query".into())]).unwrap();
        assert_eq!(out, vec![vector(7, 1.0)]);

        let request = server.request();
        assert!(request.starts_with("POST /v1/embeddings "), "{request}");
        assert!(request.contains(r#""input":["query"]"#), "{request}");
    }

    #[test]
    fn http_embedder_reports_server_errors() {
        let server = MockServer::start(500, "context too long".into());
        let err = HttpEmbedder::new(&server.url).embed(&[Input::Text("x".into())]).unwrap_err();
        assert_eq!(err.to_string(), "llama-server 500: context too long");
    }
}
