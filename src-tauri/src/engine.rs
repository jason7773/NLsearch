//! Runs llama.cpp's `llama-server` as a helper process and talks to it over
//! local HTTP. CUDA is used when an NVIDIA device is present, otherwise CPU.

use std::fs::File;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use nlsearch_core::embed::{Embedder, HttpEmbedder, Input};

pub const MODEL_FILE: &str = "embeddinggemma-2-Q8_0.gguf";
pub const MMPROJ_FILE: &str = "mmproj-embeddinggemma-2-Q8_0.gguf";

pub struct Engine {
    child: Child,
    client: HttpEmbedder,
    pub backend: &'static str,
    /// Whether the picture encoder is loaded, so pictures can be embedded.
    pub vision: bool,
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Engine {
    /// `roots` are the runtime folders to look in, in order: downloaded parts first,
    /// then what shipped with the app. The picture encoder is loaded only with `vision`:
    /// it takes more memory than the text model, and text vectors do not depend on it.
    pub fn start(roots: &[PathBuf], log_dir: &Path, vision: bool) -> Result<Engine> {
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
            match spawn(&exe, &model, vision.then_some(&mmproj), ngl, backend, log_dir) {
                Ok(engine) => return Ok(engine),
                Err(e) => errors.push(format!("{backend}: {e:#}")),
            }
        }
        if errors.is_empty() {
            bail!("no llama-server.exe found");
        }
        bail!("could not start llama-server ({})", errors.join("; "))
    }
}

impl Embedder for Engine {
    fn embed(&self, inputs: &[Input]) -> Result<Vec<Vec<f32>>> {
        self.client.embed(inputs)
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
        .no_window()
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
    mmproj: Option<&PathBuf>,
    ngl: &str,
    backend: &'static str,
    log_dir: &Path,
) -> Result<Engine> {
    let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let log = File::create(log_dir.join(format!("llama-{}.log", backend.to_lowercase())))?;

    let mut command = Command::new(exe);
    command.arg("-m").arg(model);
    if let Some(mmproj) = mmproj {
        command.arg("--mmproj").arg(mmproj);
    }
    let child = command
        .args(["--embeddings", "--pooling", "mean"])
        // The model reads each input in one pass, so -ub must cover the longest input. A
        // passage is at most 1,200 characters, about 1,300 tokens even when it is all digits,
        // and the server's working memory grows with -ub: 4096 held some 200 MB more.
        .args(["-c", "2048", "-b", "2048", "-ub", "2048"])
        .args(["-ngl", ngl])
        // The prompt cache keeps a copy of every input, pictures included, and for an
        // embedding model it counts them as taking no room, so it is never trimmed and
        // grows for as long as the server runs. Embeddings never reuse it: turn it off.
        .args(["--cache-ram", "0"])
        .args(["--host", "127.0.0.1", "--port", &port.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .no_window()
        .spawn()
        .with_context(|| format!("failed to launch {}", exe.display()))?;
    kill_with_parent(&child);
    let client = HttpEmbedder::new(&format!("http://127.0.0.1:{port}"));
    let mut engine = Engine { child, client, backend, vision: mmproj.is_some() };

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

/// Keeps helper processes from opening a console window of their own.
trait NoWindow {
    fn no_window(&mut self) -> &mut Self;
}

impl NoWindow for Command {
    #[cfg(windows)]
    fn no_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        self.creation_flags(CREATE_NO_WINDOW)
    }

    #[cfg(not(windows))]
    fn no_window(&mut self) -> &mut Self {
        self
    }
}

/// Puts the helper in a job object that is closed when this process dies, so
/// a crash or forced kill of the app never leaves llama-server holding VRAM.
#[cfg(windows)]
fn kill_with_parent(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
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

/// Elsewhere the app is only built to run the tests, so the helper is left alone.
#[cfg(not(windows))]
fn kill_with_parent(_child: &Child) {}
