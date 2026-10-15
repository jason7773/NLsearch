//! Downloads the parts that do not ship with the app: the model and the GPU backend.
//! Each file is pinned to an exact version and checked against its SHA-256 before use.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

use crate::AppState;

struct Asset {
    url: &'static str,
    file: &'static str,
    size: u64,
    sha256: &'static str,
}

/// EmbeddingGemma 2, 8-bit, at a fixed revision of the ggml-org repository.
const MODEL: &[Asset] = &[
    Asset {
        url: "https://huggingface.co/ggml-org/embeddinggemma-2-GGUF/resolve/bfcd298762cc34d0357ece5ebdd31791a3a374d8/embeddinggemma-2-Q8_0.gguf",
        file: "embeddinggemma-2-Q8_0.gguf",
        size: 309_855_456,
        sha256: "2188ac1deca4b77dffefd603c2776a9d76d9d74ec01841392982ebb840b09135",
    },
    Asset {
        url: "https://huggingface.co/ggml-org/embeddinggemma-2-GGUF/resolve/bfcd298762cc34d0357ece5ebdd31791a3a374d8/mmproj-embeddinggemma-2-Q8_0.gguf",
        file: "mmproj-embeddinggemma-2-Q8_0.gguf",
        size: 554_821_024,
        sha256: "c4a8a52691ecef40618438928bdf9e68379b854e24166f292592353db0aab64f",
    },
];

/// llama.cpp b11552 built for CUDA 12.4, plus the CUDA libraries it needs. The build
/// number matches the CPU backend the app ships with.
const GPU: &[Asset] = &[
    Asset {
        url: "https://github.com/ggml-org/llama.cpp/releases/download/b11552/llama-b11552-bin-win-cuda-12.4-x64.zip",
        file: "llama-b11552-bin-win-cuda-12.4-x64.zip",
        size: 265_234_947,
        sha256: "e48331718aa82aa34571cfad8510e6d6b9a8e4dad11a73e065f94f212e280256",
    },
    Asset {
        url: "https://github.com/ggml-org/llama.cpp/releases/download/b11552/cudart-llama-bin-win-cuda-12.4-x64.zip",
        file: "cudart-llama-bin-win-cuda-12.4-x64.zip",
        size: 391_443_627,
        sha256: "8c79a9b226de4b3cacfd1f83d24f962d0773be79f1e7b75c6af4ded7e32ae1d6",
    },
];

fn assets(component: &str) -> Option<&'static [Asset]> {
    match component {
        "model" => Some(MODEL),
        "gpu" => Some(GPU),
        _ => None,
    }
}

pub fn download_size(component: &str) -> u64 {
    assets(component).map_or(0, |list| list.iter().map(|a| a.size).sum())
}

/// Progress of the download in flight, or the outcome of the last one.
#[derive(Clone, Default, Serialize)]
pub struct Download {
    pub component: String,
    pub running: bool,
    pub done: u64,
    pub total: u64,
    pub step: String,
    pub error: Option<String>,
    pub cancelled: bool,
}

/// True when an NVIDIA driver is installed, which is what the GPU backend needs.
pub fn nvidia_present() -> bool {
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| "C:\\Windows".into());
    ["nvml.dll", "nvidia-smi.exe"].iter().any(|f| system.join("System32").join(f).exists())
}

/// Starts downloading `component` ("model" or "gpu") in the background.
pub fn start(app: AppHandle, component: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    if assets(&component).is_none() {
        return Err(format!("unknown component: {component}"));
    }
    {
        let mut download = state.download.lock().unwrap();
        if download.running {
            return Err("another download is in progress".into());
        }
        *download = Download {
            component: component.clone(),
            running: true,
            total: download_size(&component),
            step: "Starting".into(),
            ..Default::default()
        };
    }
    state.download_cancel.store(false, Ordering::SeqCst);

    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let result = run(&state, &component);
        let cancelled = state.download_cancel.load(Ordering::SeqCst);
        let mut download = state.download.lock().unwrap();
        download.running = false;
        download.cancelled = cancelled;
        // Stopping on request is not a failure; the partial file is kept for next time.
        download.error = match result {
            Err(e) if !cancelled => Some(format!("{e:#}")),
            _ => None,
        };
    });
    Ok(())
}

fn run(state: &AppState, component: &str) -> Result<()> {
    let list = assets(component).context("unknown component")?;
    // The first runtime folder is the writable one, inside the data folder.
    let root = &state.runtimes[0];
    let staging = root.join("downloads");
    fs::create_dir_all(&staging)?;

    let mut before = 0;
    for asset in list {
        fetch(state, asset, &staging.join(asset.file), before)?;
        before += asset.size;
    }

    set_step(state, "Installing");
    match component {
        "model" => {
            let models = root.join("models");
            fs::create_dir_all(&models)?;
            for asset in list {
                fs::rename(staging.join(asset.file), models.join(asset.file))?;
            }
        }
        _ => {
            // Unpack beside the final location, then swap in, so a half-written
            // backend is never picked up.
            let partial = root.join("llama-cuda.partial");
            let target = root.join("llama-cuda");
            let _ = fs::remove_dir_all(&partial);
            fs::create_dir_all(&partial)?;
            for asset in list {
                unpack_server(&staging.join(asset.file), &partial)?;
            }
            let _ = fs::remove_dir_all(&target);
            fs::rename(&partial, &target)?;
            for asset in list {
                let _ = fs::remove_file(staging.join(asset.file));
            }
            // The next search or indexing pass starts the model on the GPU.
            state.restart_engine();
        }
    }
    let _ = fs::remove_dir(&staging);
    Ok(())
}

fn set_step(state: &AppState, step: &str) {
    state.download.lock().unwrap().step = step.into();
}

/// Downloads one file to `dest`, resuming a partial download, and verifies it.
fn fetch(state: &AppState, asset: &Asset, dest: &Path, before: u64) -> Result<()> {
    if dest.exists() && fs::metadata(dest)?.len() == asset.size {
        state.download.lock().unwrap().done = before + asset.size;
        return Ok(());
    }
    let part = dest.with_extension("part");
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .timeout_read(Duration::from_secs(60))
        .build();

    let mut last_error = None;
    for attempt in 0..10 {
        let mut have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if have > asset.size {
            fs::remove_file(&part)?;
            have = 0;
        }
        if have == asset.size {
            break;
        }
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(2));
        }
        set_step(state, &format!("Downloading {}", asset.file));

        let mut request = agent.get(asset.url).set("User-Agent", "NLsearch");
        if have > 0 {
            request = request.set("Range", &format!("bytes={have}-"));
        }
        let response = match request.call() {
            Ok(response) => response,
            Err(e) => {
                last_error = Some(anyhow::anyhow!(e));
                continue;
            }
        };
        // A server that ignores the range sends the whole file again.
        let resumed = response.status() == 206;
        let mut file = OpenOptions::new().create(true).write(true).append(resumed).truncate(!resumed).open(&part)?;
        if !resumed {
            have = 0;
        }

        let mut reader = response.into_reader();
        let mut buffer = vec![0u8; 256 * 1024];
        loop {
            if state.download_cancel.load(Ordering::SeqCst) {
                bail!("cancelled");
            }
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    file.write_all(&buffer[..n])?;
                    have += n as u64;
                    state.download.lock().unwrap().done = before + have;
                }
                // A dropped connection: the next attempt carries on from here.
                Err(e) => {
                    last_error = Some(anyhow::anyhow!(e));
                    break;
                }
            }
        }
    }

    let have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have != asset.size {
        return Err(last_error.unwrap_or_else(|| anyhow::anyhow!("incomplete download")))
            .with_context(|| format!("could not download {}", asset.file));
    }

    set_step(state, &format!("Verifying {}", asset.file));
    let mut hasher = Sha256::new();
    std::io::copy(&mut File::open(&part)?, &mut hasher)?;
    let digest: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if digest != asset.sha256 {
        fs::remove_file(&part)?;
        bail!("{} failed its integrity check; please try again", asset.file);
    }
    fs::rename(&part, dest)?;
    Ok(())
}

/// Extracts the server and its libraries from a llama.cpp archive into `dir`. Entries
/// are written by file name only, so nothing can land outside `dir`.
fn unpack_server(archive: &Path, dir: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(File::open(archive)?)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if !entry.is_file() {
            continue;
        }
        let Some(name) = Path::new(entry.name()).file_name().map(|n| n.to_owned()) else { continue };
        let lower = name.to_string_lossy().to_lowercase();
        if lower == "llama-server.exe" || lower.ends_with(".dll") {
            std::io::copy(&mut entry, &mut File::create(dir.join(name))?)?;
        }
    }
    Ok(())
}
