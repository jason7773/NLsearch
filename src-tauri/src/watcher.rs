//! Watches the indexed folders and starts a pass when files in them change, so new and
//! edited files become searchable without a Rescan.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::Result;
use nlsearch_core::db::{self, Settings};
use nlsearch_core::extract::{kind_for, Exclusions};
use notify::{EventKind, RecursiveMode, Watcher};
use tauri::{AppHandle, Manager};

use crate::{indexer, AppState};

/// Changes usually come in bursts (a copy, an unzip, a save): wait for this much quiet.
const QUIET: Duration = Duration::from_secs(3);
/// A file written to non-stop, such as a log, still gets picked up this often.
const MAX_WAIT: Duration = Duration::from_secs(60);

/// Starts the thread that turns changes into indexing passes, and watches the folders.
pub fn start(app: AppHandle) {
    let (sender, receiver) = mpsc::channel::<()>();
    *app.state::<AppState>().watch_sender.lock().unwrap() = Some(sender);
    let handle = app.clone();
    std::thread::spawn(move || {
        while receiver.recv().is_ok() {
            let first = Instant::now();
            loop {
                match receiver.recv_timeout(QUIET) {
                    Ok(()) if first.elapsed() < MAX_WAIT => continue,
                    Ok(()) | Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            indexer::start(handle.clone());
        }
    });
    refresh(&app);
}

/// Watches the folders currently in the index, with the current exclusions. Called
/// again whenever either changes.
pub fn refresh(app: &AppHandle) {
    if let Err(e) = try_refresh(app) {
        eprintln!("could not watch folders: {e:#}");
    }
}

fn try_refresh(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let Some(sender) = state.watch_sender.lock().unwrap().clone() else { return Ok(()) };
    let conn = db::open(&state.db_path)?;
    let roots: Vec<PathBuf> = conn
        .prepare("SELECT path FROM folders")?
        .query_map([], |r| r.get::<_, String>(0))?
        .map(|p| p.map(PathBuf::from))
        .collect::<rusqlite::Result<_>>()?;
    let exclusions = Exclusions::new(&Settings::load(&conn)?.exclude);

    let watched = roots.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        // Reading a file, which indexing itself does, is not a change.
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        if event.paths.iter().any(|path| relevant(&watched, &exclusions, path)) {
            let _ = sender.send(());
        }
    })?;
    for root in &roots {
        // A folder on a drive that is not connected is simply not watched.
        let _ = watcher.watch(root, RecursiveMode::Recursive);
    }
    // Replacing the old watcher stops it.
    *state.watcher.lock().unwrap() = Some(watcher);
    Ok(())
}

/// Whether a change at `path` can affect the index: a file NLsearch reads, or a folder,
/// since renaming or deleting one moves the files in it.
fn relevant(roots: &[PathBuf], exclusions: &Exclusions, path: &Path) -> bool {
    let Some(root) = roots.iter().find(|root| path.starts_with(root)) else { return false };
    !exclusions.excludes(root, path) && (kind_for(path).is_some() || path.extension().is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_indexable_files_and_folders_inside_the_roots_count() {
        let root = std::env::temp_dir().join("nlsearch-watch");
        let roots = vec![root.clone()];
        let exclusions = Exclusions::new(&["Archive".to_string()]);
        assert!(relevant(&roots, &exclusions, &root.join("notes/plan.docx")));
        assert!(relevant(&roots, &exclusions, &root.join("New folder")));
        assert!(!relevant(&roots, &exclusions, &root.join("setup.exe")));
        assert!(!relevant(&roots, &exclusions, &root.join("Archive/old.txt")));
        assert!(!relevant(&roots, &exclusions, &root.join(".git/index.txt")));
        assert!(!relevant(&roots, &exclusions, &std::env::temp_dir().join("elsewhere.txt")));
    }
}
