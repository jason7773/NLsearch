//! Stand-ins for the model and llama-server, so tests run without either.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;

use anyhow::{bail, Result};

use crate::embed::{normalize, Embedder, Input, DIM};

/// A model that only knows words: each word lights up one dimension, so passages
/// sharing words with the query score highest. Images embed as their (fake) bytes.
pub struct FakeEmbedder {
    /// Any input containing this text fails, as a real model does on a bad input.
    pub fail_on: Option<String>,
}

impl FakeEmbedder {
    pub fn new() -> FakeEmbedder {
        FakeEmbedder { fail_on: None }
    }

    pub fn vector(text: &str) -> Vec<f32> {
        let mut v = vec![0.0; DIM];
        for word in text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
            let word = word.to_lowercase();
            // Prompt scaffolding is shared by every input; it would only add noise.
            if ["task", "search", "result", "query", "title", "text"].contains(&word.as_str()) {
                continue;
            }
            let hash = word.bytes().fold(5381usize, |h, b| h.wrapping_mul(33) ^ b as usize);
            v[hash % DIM] += 1.0;
        }
        normalize(&mut v);
        v
    }
}

impl Embedder for FakeEmbedder {
    fn embed(&self, inputs: &[Input]) -> Result<Vec<Vec<f32>>> {
        inputs
            .iter()
            .map(|input| {
                let text = match input {
                    Input::Text(t) => t.as_str(),
                    Input::Image(_) => "image",
                };
                if let Some(bad) = &self.fail_on {
                    if text.contains(bad.as_str()) {
                        bail!("llama-server 500: bad input");
                    }
                }
                Ok(FakeEmbedder::vector(text))
            })
            .collect()
    }
}

/// An HTTP server on localhost that answers one request with a fixed reply.
pub struct MockServer {
    pub url: String,
    request: mpsc::Receiver<String>,
}

impl MockServer {
    pub fn start(status: u16, body: String) -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut head = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
                head.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            let mut payload = vec![0; length];
            reader.read_exact(&mut payload).unwrap();
            head.push_str(&String::from_utf8_lossy(&payload));
            let _ = tx.send(head);

            let mut stream = reader.into_inner();
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        });
        MockServer { url, request: rx }
    }

    /// The request the server received: head and body.
    pub fn request(&self) -> String {
        self.request.recv().unwrap()
    }
}
