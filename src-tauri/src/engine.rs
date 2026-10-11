//! Runs llama.cpp's `llama-server` as a helper process and talks to it over
//! local HTTP. CUDA is used when an NVIDIA device is present, otherwise CPU.

use std::fs::File;
use std::net::TcpListener;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

pub const MODEL_FILE: &str = "embeddinggemma-2-Q8_0.gguf";
pub const MMPROJ_FILE: &str = "mmproj-embeddinggemma-2-Q8_0.gguf";
/// Stored with the index so vectors from a different model are never mixed.
pub const MODEL_ID: &str = "embeddinggemma-2-Q8_0";
pub const DIM: usize = 768;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub enum Input {
    Text(String),
    /// Base64-encoded JPEG.
    Image(String),
}

pub struct Engine {
    child: Child,
    port: u16,
    pub backend: &'static str,
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Engine {
    /// `roots` are the runtime folders to look in, in order: downloaded parts first,
    /// then what shipped with the app.
    pub fn start(roots: &[PathBuf], log_dir: &Path) -> Result<Engine> {
        let find = |file: &str| {
            locate(roots, &format!("models/{file}")).with_context(|| format!("model file missing: {file}"))
        };
        let model = find(MODEL_FILE)?;
        let mmproj = find(MMPROJ_FILE)?;

        let mut errors = Vec::new();
        for (backend, dir, ngl) in [("CUDA", "llama-cuda", "99"), ("CPU", "llama-cpu", "0")] {
            let Some(exe) = locate(roots, &format!("{dir}/llama-server.exe")) else { continue };
            if backend == "CUDA" && !has_cuda_device(&exe) {
                continue;
            }
            match spawn(&exe, &model, &mmproj, ngl, backend, log_dir) {
                Ok(engine) => return Ok(engine),
                Err(e) => errors.push(format!("{backend}: {e:#}")),
            }
        }
        if errors.is_empty() {
            bail!("no llama-server.exe found");
        }
        bail!("could not start llama-server ({})", errors.join("; "))
    }

    /// Returns one L2-normalised vector per input, in input order.
    pub fn embed(&self, inputs: &[Input]) -> Result<Vec<Vec<f32>>> {
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

        let resp: Value = ureq::post(&format!("http://127.0.0.1:{}/v1/embeddings", self.port))
            .timeout(Duration::from_secs(600))
            .send_json(json!({ "input": items, "encoding_format": "float" }))
            .map_err(|e| match e {
                ureq::Error::Status(code, r) => {
                    anyhow!("llama-server {code}: {}", r.into_string().unwrap_or_default())
                }
                other => anyhow!(other),
            })?
            .into_json()?;

        let data = resp["data"].as_array().context("embeddings response has no data")?;
        let mut out = vec![Vec::new(); inputs.len()];
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
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                v.iter_mut().for_each(|x| *x /= norm);
            }
            *out.get_mut(idx).context("embedding index out of range")? = v;
        }
        if out.iter().any(|v| v.is_empty()) {
            bail!("llama-server returned fewer embeddings than inputs");
        }
        Ok(out)
    }
}

/// The first of `roots` that contains `relative`, as a full path.
pub fn locate(roots: &[PathBuf], relative: &str) -> Option<PathBuf> {
    roots.iter().map(|root| root.join(relative)).find(|path| path.exists())
}

pub fn model_installed(roots: &[PathBuf]) -> bool {
    [MODEL_FILE, MMPROJ_FILE].iter().all(|f| locate(roots, &format!("models/{f}")).is_some())
}

pub fn gpu_backend_installed(roots: &[PathBuf]) -> bool {
    locate(roots, "llama-cuda/llama-server.exe").is_some()
}

fn has_cuda_device(exe: &Path) -> bool {
    Command::new(exe)
        .arg("--list-devices")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| {
            let text = [o.stdout, o.stderr].concat();
            String::from_utf8_lossy(&text).contains("CUDA0")
        })
        .unwrap_or(false)
}

fn spawn(
    exe: &Path,
    model: &PathBuf,
    mmproj: &PathBuf,
    ngl: &str,
    backend: &'static str,
    log_dir: &Path,
) -> Result<Engine> {
    let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let log = File::create(log_dir.join(format!("llama-{}.log", backend.to_lowercase())))?;

    let child = Command::new(exe)
        .arg("-m").arg(model)
        .arg("--mmproj").arg(mmproj)
        .args(["--embeddings", "--pooling", "mean"])
        // The model reads each input in one pass, so -ub must cover the longest input.
        .args(["-c", "4096", "-b", "4096", "-ub", "4096"])
        .args(["-ngl", ngl])
        .args(["--host", "127.0.0.1", "--port", &port.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .with_context(|| format!("failed to launch {}", exe.display()))?;
    kill_with_parent(&child);
    let mut engine = Engine { child, port, backend };

    let health = format!("http://127.0.0.1:{port}/health");
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Some(status) = engine.child.try_wait()? {
            bail!("llama-server exited during startup ({status})");
        }
        if ureq::get(&health).timeout(Duration::from_secs(2)).call().is_ok() {
            return Ok(engine);
        }
        if Instant::now() > deadline {
            bail!("llama-server did not become ready in time");
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Puts the helper in a job object that is closed when this process dies, so
/// a crash or forced kill of the app never leaves llama-server holding VRAM.
fn kill_with_parent(child: &Child) {
    use windows_sys::Win32::System::JobObjects::*;

    static JOB: OnceLock<usize> = OnceLock::new();
    let job = *JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        job as usize
    });
    unsafe {
        AssignProcessToJobObject(job as _, child.as_raw_handle() as _);
    }
}
