//! Walks the indexed folders, extracts text passages, PDF pages and images,
//! embeds them and stores the vectors. Unchanged files are skipped.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Mutex};

use anyhow::Result;
use nlsearch_core::db::{self, Settings};
use nlsearch_core::extract::{extract, scan, Candidate, Exclusions, Item, Ocr};
use nlsearch_core::index::{self, EMBED_BATCH};
use rusqlite::Connection;
use tauri::{AppHandle, Emitter, Manager};

use crate::engine::Engine;
use crate::AppState;

struct Job {
    folder_id: i64,
    file: Candidate,
    items: Vec<Item>,
    /// Preview JPEG, for images.
    thumb: Option<Vec<u8>>,
}

/// Runs passes until no rescan is pending. Returns immediately if one is already running.
pub fn start(app: AppHandle) {
    let state = app.state::<AppState>();
    if state.indexing.swap(true, Ordering::SeqCst) {
        state.rescan.store(true, Ordering::SeqCst);
        return;
    }
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        loop {
            let result = run_pass(&app, &state);
            {
                let mut p = state.progress.lock().unwrap();
                p.running = false;
                p.stopped = state.cancel.load(Ordering::SeqCst);
                p.current.clear();
                p.error = result.err().map(|e| format!("{e:#}"));
            }
            if !state.rescan.swap(false, Ordering::SeqCst) {
                break;
            }
        }
        state.indexing.store(false, Ordering::SeqCst);
        emit(&app, &state);
    });
}

fn emit(app: &AppHandle, state: &AppState) {
    let _ = app.emit("progress", state.progress.lock().unwrap().clone());
}

fn run_pass(app: &AppHandle, state: &AppState) -> Result<()> {
    state.cancel.store(false, Ordering::SeqCst);
    let cancelled = || state.cancel.load(Ordering::SeqCst);
    let conn = db::open(&state.db_path)?;
    let chunks = |conn: &Connection| conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get::<_, i64>(0));
    let before = chunks(&conn)?;
    index::tidy(&conn)?;
    let settings = Settings::load(&conn)?;
    let exclusions = Exclusions::new(&settings.exclude);
    // OCR counts as on only when Windows can do it, so files are not redone every pass
    // on a PC without an OCR language.
    let ocr_engine = if settings.ocr { crate::ocr::engine() } else { None };
    let ocr: Option<&dyn Ocr> = ocr_engine.as_ref().map(|o| o as &dyn Ocr);
    let ocr_on = ocr.is_some();
    let folders: Vec<(i64, String)> = conn
        .prepare("SELECT id, path FROM folders")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;

    {
        let mut p = state.progress.lock().unwrap();
        *p = Default::default();
        p.running = true;
        p.phase = "Scanning".into();
    }
    emit(app, state);

    // Work out what changed before loading the model.
    let mut todo: Vec<(i64, Candidate)> = Vec::new();
    for (folder_id, folder) in &folders {
        let changed = index::plan_folder(&conn, *folder_id, scan(Path::new(folder), &exclusions), ocr_on)?;
        todo.extend(changed.into_iter().map(|file| (*folder_id, file)));
    }
    // Tidying and planning only remove passages, and the cache is read again whole only
    // when they did: new passages are appended as they are written. A pass that finds
    // nothing changed, which is most of what the folder watcher starts, keeps the cache.
    if chunks(&conn)? != before {
        state.cache_dirty.store(true, Ordering::SeqCst);
    }

    if todo.is_empty() || cancelled() {
        return Ok(());
    }
    {
        let mut p = state.progress.lock().unwrap();
        p.phase = "Loading model".into();
        p.total = todo.len();
    }
    emit(app, state);
    // Text needs only the text model; the picture encoder is loaded when there are pictures.
    let engine = state.engine(todo.iter().any(|(_, file)| file.kind == "image"))?;
    state.progress.lock().unwrap().phase = "Indexing".into();

    // Reading files (decoding photos above all) is the slow part, so it runs on several
    // cores while this thread sends finished files to the model and writes the results.
    let queue = Mutex::new(todo.into_iter());
    let (sender, receiver) = mpsc::sync_channel::<Job>(extract_threads() * 2);
    std::thread::scope(|scope| {
        for _ in 0..extract_threads() {
            let sender = sender.clone();
            let queue = &queue;
            scope.spawn(move || loop {
                if state.cancel.load(Ordering::SeqCst) {
                    break;
                }
                let Some((folder_id, file)) = queue.lock().unwrap().next() else { break };
                // A file that cannot be read is recorded with nothing to search.
                let (items, thumb) = catch_unwind(AssertUnwindSafe(|| extract(&file, ocr)))
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or_default();
                if sender.send(Job { folder_id, file, items, thumb }).is_err() {
                    break;
                }
            });
        }
        drop(sender);

        let mut jobs: Vec<Job> = Vec::new();
        let mut pending = 0;
        let mut result = Ok(());
        for job in &receiver {
            if cancelled() {
                break;
            }
            state.progress.lock().unwrap().current = job.file.path.to_string_lossy().into_owned();
            pending += job.items.len();
            jobs.push(job);
            if pending >= EMBED_BATCH {
                result = flush(&conn, &engine, state, &mut jobs, ocr_on);
                pending = 0;
                emit(app, state);
                if result.is_err() {
                    break;
                }
            }
        }
        if result.is_ok() {
            result = flush(&conn, &engine, state, &mut jobs, ocr_on);
        }
        // Readers still waiting to hand over a file stop once nobody is listening.
        drop(receiver);
        result
    })?;
    emit(app, state);
    Ok(())
}

/// How many files are read at once: half the logical cores, so the rest of the PC
/// stays responsive. NLSEARCH_INDEX_THREADS overrides it.
fn extract_threads() -> usize {
    if let Some(n) = std::env::var("NLSEARCH_INDEX_THREADS").ok().and_then(|v| v.parse().ok()) {
        return std::cmp::max(n, 1);
    }
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    (cores / 2).clamp(1, 8)
}

/// Embeds every item of the queued jobs and writes each file in one transaction.
fn flush(conn: &Connection, engine: &Engine, state: &AppState, jobs: &mut Vec<Job>, ocr: bool) -> Result<()> {
    for job in jobs.drain(..) {
        state.touch();
        let proceed = || {
            if state.cancel.load(Ordering::SeqCst) {
                return false;
            }
            // Searches go first: wait while one is asking the model for its query vector.
            while state.searching.load(Ordering::SeqCst) > 0 {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            true
        };
        let Some(rows) = index::embed_items(engine, job.items, proceed) else { return Ok(()) };

        // The cache lock is held across the commit so a concurrent reload either sees
        // all of this file's vectors in the database or gets them appended, never both.
        let mut cache = state.cache.lock().unwrap();
        let (file_id, added) = index::write_file(conn, job.folder_id, &job.file, rows, job.thumb.as_deref(), ocr)?;
        // A cache already marked stale is reloaded whole by the next search.
        if !state.cache_dirty.load(Ordering::SeqCst) {
            let kind = db::kind_code(job.file.kind);
            for (id, vec) in &added {
                cache.push(*id, file_id, kind, vec);
            }
        }
        drop(cache);
        state.progress.lock().unwrap().done += 1;
    }
    Ok(())
}
