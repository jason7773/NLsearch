//! Walks the indexed folders, extracts text passages, PDF pages and images,
//! embeds them and stores the vectors. Unchanged files are skipped.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Mutex};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use image::ImageDecoder;
use base64::Engine as _;
use rusqlite::{params, Connection};
use tauri::{AppHandle, Emitter, Manager};

use crate::db;
use crate::engine::{Engine, Input};
use crate::text::{self, chunk_text};
use crate::AppState;

/// Inputs per embedding request. Kept small so a search never waits long behind one.
const EMBED_BATCH: usize = 8;
/// Longest image side sent to the model.
const IMAGE_MAX_SIDE: u32 = 896;
/// Longest side of the stored preview shown in results.
const THUMB_MAX_SIDE: u32 = 360;

const MAX_TEXT_BYTES: u64 = 5 * 1024 * 1024;
const MAX_PDF_BYTES: u64 = 100 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 50 * 1024 * 1024;

const SKIP_DIRS: &[&str] = &[
    "node_modules", "target", "__pycache__", "$RECYCLE.BIN", "System Volume Information",
    "AppData", "venv", "dist", "build",
];

fn kind_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "txt" | "md" | "markdown" | "rst" | "log" | "csv" | "html" | "htm" | "docx" => "text",
        "pdf" => "pdf",
        "jpg" | "jpeg" | "png" | "webp" | "bmp" | "gif" => "image",
        _ => return None,
    })
}

struct Candidate {
    path: PathBuf,
    mtime: i64,
    size: u64,
    kind: &'static str,
}

struct Item {
    page: Option<i64>,
    /// Stored for snippets and keyword search.
    text: String,
    input: Input,
}

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
    // Files whose folder was removed while they were being written.
    conn.execute_batch(
        "DELETE FROM chunks WHERE file_id IN
             (SELECT id FROM files WHERE folder_id NOT IN (SELECT id FROM folders));
         DELETE FROM files WHERE folder_id NOT IN (SELECT id FROM folders);
         DELETE FROM thumbs WHERE file_id NOT IN (SELECT id FROM files);",
    )?;
    db::merge_nested_folders(&conn)?;
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
        let mut known: HashMap<String, (i64, i64, u64)> = conn
            .prepare("SELECT path, id, mtime, size FROM files WHERE folder_id = ?1")?
            .query_map(params![folder_id], |r| {
                Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get::<_, i64>(3)? as u64)))
            })?
            .collect::<rusqlite::Result<_>>()?;

        for file in scan(Path::new(folder)) {
            let key = file.path.to_string_lossy().into_owned();
            match known.remove(&key) {
                Some((_, mtime, size)) if mtime == file.mtime && size == file.size => {}
                Some((id, ..)) => {
                    db::delete_file(&conn, id)?;
                    todo.push((*folder_id, file));
                }
                None => todo.push((*folder_id, file)),
            }
        }
        // Whatever is left no longer exists on disk.
        for (id, ..) in known.into_values() {
            db::delete_file(&conn, id)?;
        }
        let pending = todo.iter().filter(|(id, _)| id == folder_id).count();
        conn.execute(
            "UPDATE folders SET pending = ?1 WHERE id = ?2",
            params![pending as i64, folder_id],
        )?;
    }
    state.cache_dirty.store(true, Ordering::SeqCst);

    if todo.is_empty() || cancelled() {
        return Ok(());
    }
    {
        let mut p = state.progress.lock().unwrap();
        p.phase = "Loading model".into();
        p.total = todo.len();
    }
    emit(app, state);
    let engine = state.engine()?;
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
                let (items, thumb) = catch_unwind(AssertUnwindSafe(|| extract(&file)))
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
                result = flush(&conn, &engine, state, &mut jobs);
                pending = 0;
                emit(app, state);
                if result.is_err() {
                    break;
                }
            }
        }
        if result.is_ok() {
            result = flush(&conn, &engine, state, &mut jobs);
        }
        // Readers still waiting to hand over a file stop once nobody is listening.
        drop(receiver);
        result
    })?;
    emit(app, state);
    Ok(())
}

/// How many files are read at once: half the logical cores, so the rest of the PC
/// stays responsive. FILEDIG_INDEX_THREADS overrides it.
fn extract_threads() -> usize {
    if let Some(n) = std::env::var("FILEDIG_INDEX_THREADS").ok().and_then(|v| v.parse().ok()) {
        return std::cmp::max(n, 1);
    }
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    (cores / 2).clamp(1, 8)
}

/// Embeds every item of the queued jobs and writes each file in one transaction.
fn flush(conn: &Connection, engine: &Engine, state: &AppState, jobs: &mut Vec<Job>) -> Result<()> {
    for job in jobs.drain(..) {
        state.touch();
        let mut rows: Vec<(Option<i64>, String, Vec<f32>)> = Vec::new();
        let mut items = job.items.into_iter().peekable();
        while items.peek().is_some() {
            // A stopped file is not written at all, so the next pass redoes it whole.
            if state.cancel.load(Ordering::SeqCst) {
                return Ok(());
            }
            // Searches go first: wait while one is asking the model for its query vector.
            while state.searching.load(Ordering::SeqCst) > 0 {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let batch: Vec<Item> = items.by_ref().take(EMBED_BATCH).collect();
            let (meta, inputs): (Vec<_>, Vec<_>) =
                batch.into_iter().map(|i| ((i.page, i.text), i.input)).unzip();
            match engine.embed(&inputs) {
                Ok(vecs) => rows.extend(meta.into_iter().zip(vecs).map(|((p, t), v)| (p, t, v))),
                // One bad input fails the whole request: retry singly and drop the culprit.
                Err(_) => {
                    for ((page, text), input) in meta.into_iter().zip(inputs) {
                        if let Ok(mut v) = engine.embed(std::slice::from_ref(&input)) {
                            rows.push((page, text, v.remove(0)));
                        }
                    }
                }
            }
        }

        // The cache lock is held across the commit so a concurrent reload either sees
        // all of this file's vectors in the database or gets them appended, never both.
        let mut cache = state.cache.lock().unwrap();
        let mut added: Vec<(i64, Vec<f32>)> = Vec::new();

        // Files with nothing indexable are still recorded so they are not retried every pass.
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO files (folder_id, path, mtime, size, kind) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                job.folder_id,
                job.file.path.to_string_lossy(),
                job.file.mtime,
                job.file.size as i64,
                job.file.kind
            ],
        )?;
        let file_id = tx.last_insert_rowid();
        for (page, text, vec) in rows {
            tx.execute(
                "INSERT INTO chunks (file_id, page, text, emb) VALUES (?1, ?2, ?3, ?4)",
                params![file_id, page, text, db::vec_to_blob(&vec)],
            )?;
            added.push((tx.last_insert_rowid(), vec));
        }
        if let Some(jpeg) = &job.thumb {
            tx.execute(
                "INSERT OR REPLACE INTO thumbs (file_id, jpeg) VALUES (?1, ?2)",
                params![file_id, jpeg],
            )?;
        }
        tx.execute(
            "UPDATE folders SET pending = MAX(pending - 1, 0) WHERE id = ?1",
            params![job.folder_id],
        )?;
        tx.commit()?;
        // A cache already marked stale is reloaded whole by the next search.
        if !state.cache_dirty.load(Ordering::SeqCst) {
            let kind = db::kind_code(job.file.kind);
            for (id, vec) in &added {
                cache.push(*id, kind, vec);
            }
        }
        drop(cache);
        state.progress.lock().unwrap().done += 1;
    }
    Ok(())
}

fn scan(root: &Path) -> Vec<Candidate> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            e.depth() == 0 || !(name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()))
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let kind = kind_for(e.path())?;
            let meta = e.metadata().ok()?;
            let limit = match kind {
                "pdf" => MAX_PDF_BYTES,
                "image" => MAX_IMAGE_BYTES,
                _ => MAX_TEXT_BYTES,
            };
            if meta.len() == 0 || meta.len() > limit {
                return None;
            }
            let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
            Some(Candidate { path: e.into_path(), mtime, size: meta.len(), kind })
        })
        .collect()
}

/// Decodes an image upright: cameras store portrait photos rotated, with the turn in EXIF.
fn open_image(path: &Path) -> Result<image::DynamicImage> {
    let mut decoder = image::ImageReader::open(path)?.with_guessed_format()?.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut img = image::DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img)
}

fn encode_jpeg(img: &image::DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut jpeg), quality)
        .encode_image(&img.to_rgb8())?;
    Ok(jpeg)
}

/// Small preview of an image file, for files indexed before previews were stored.
pub fn make_thumb(path: &Path) -> Result<Vec<u8>> {
    encode_jpeg(&open_image(path)?.thumbnail(THUMB_MAX_SIDE, THUMB_MAX_SIDE), 80)
}

/// The passages or image to embed for a file, plus a preview for images.
fn extract(file: &Candidate) -> Result<(Vec<Item>, Option<Vec<u8>>)> {
    let name = file.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let passage = |page: Option<i64>, text: String| Item {
        page,
        input: Input::Text(format!("title: {name} | text: {text}")),
        text,
    };

    match file.kind {
        "image" => {
            let model_size = open_image(&file.path)?.thumbnail(IMAGE_MAX_SIDE, IMAGE_MAX_SIDE);
            let b64 = base64::engine::general_purpose::STANDARD.encode(encode_jpeg(&model_size, 85)?);
            let thumb = encode_jpeg(&model_size.thumbnail(THUMB_MAX_SIDE, THUMB_MAX_SIDE), 80)?;
            // The file name is what keyword search matches for an image.
            let item = Item { page: None, text: name.clone(), input: Input::Image(b64) };
            Ok((vec![item], Some(thumb)))
        }
        "pdf" => {
            // pdf-extract panics on some malformed files.
            let pages = catch_unwind(AssertUnwindSafe(|| pdf_extract::extract_text_by_pages(&file.path)))
                .map_err(|_| anyhow::anyhow!("pdf parser panicked"))??;
            let items = pages
                .iter()
                .enumerate()
                .flat_map(|(i, text)| {
                    chunk_text(text).into_iter().map(move |c| (Some(i as i64 + 1), c))
                })
                .map(|(page, c)| passage(page, c))
                .collect();
            Ok((items, None))
        }
        _ => {
            let ext = file.path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            let text = match ext.as_str() {
                "docx" => docx_text(&file.path)?,
                "html" | "htm" => {
                    let html = read_text(&file.path)?;
                    strip_tags(&drop_blocks(&drop_blocks(&html, "script"), "style"))
                }
                _ => read_text(&file.path)?,
            };
            Ok((chunk_text(&text).into_iter().map(|c| passage(None, c)).collect(), None))
        }
    }
}

fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    text::decode(&bytes)
}

fn docx_text(path: &Path) -> Result<String> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let mut xml = String::new();
    archive.by_name("word/document.xml").context("not a docx")?.read_to_string(&mut xml)?;
    Ok(strip_tags(&xml.replace("</w:p>", "\n")))
}

/// Removes `<tag ...>...</tag>` blocks, contents included.
fn drop_blocks(html: &str, tag: &str) -> String {
    // ASCII lowercasing keeps byte offsets identical to `html`.
    let lower = html.to_ascii_lowercase();
    let (open, close) = (format!("<{tag}"), format!("</{tag}>"));
    let mut out = String::with_capacity(html.len());
    let mut pos = 0;
    while let Some(start) = lower[pos..].find(&open) {
        out.push_str(&html[pos..pos + start]);
        pos = match lower[pos + start..].find(&close) {
            Some(end) => pos + start + end + close.len(),
            None => html.len(),
        };
    }
    out.push_str(&html[pos..]);
    out
}

fn strip_tags(markup: &str) -> String {
    let mut out = String::with_capacity(markup.len());
    let mut in_tag = false;
    for c in markup.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}
