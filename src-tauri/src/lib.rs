mod db;
mod engine;
mod indexer;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Condvar;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::params;
use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_opener::OpenerExt;

use db::VecCache;
use engine::{Engine, Input};

const HOTKEY: &str = "ctrl+shift+space";
/// The helper process is stopped after this long without use, freeing VRAM.
const ENGINE_IDLE: Duration = Duration::from_secs(300);

#[derive(Clone, Default, Serialize)]
pub struct Progress {
    pub running: bool,
    pub phase: String,
    pub done: usize,
    pub total: usize,
    pub current: String,
    pub error: Option<String>,
    /// The last pass was stopped by the user before it finished.
    pub stopped: bool,
}

pub struct AppState {
    pub db_path: PathBuf,
    runtime: PathBuf,
    log_dir: PathBuf,
    engine: Mutex<Option<Arc<Engine>>>,
    last_used: Mutex<Instant>,
    backend: Mutex<String>,
    pub progress: Mutex<Progress>,
    pub indexing: AtomicBool,
    pub rescan: AtomicBool,
    pub cancel: AtomicBool,
    pub cache: Mutex<VecCache>,
    pub cache_dirty: AtomicBool,
    /// Searches currently waiting on the model; the indexer yields to them.
    pub searching: AtomicUsize,
}

impl AppState {
    /// Returns the running engine, starting it first if needed.
    pub fn engine(&self) -> anyhow::Result<Arc<Engine>> {
        let mut slot = self.engine.lock().unwrap();
        self.touch();
        if let Some(engine) = slot.as_ref() {
            return Ok(engine.clone());
        }
        let engine = Arc::new(Engine::start(&self.runtime, &self.log_dir)?);
        *self.backend.lock().unwrap() = engine.backend.to_string();
        *slot = Some(engine.clone());
        Ok(engine)
    }

    pub fn touch(&self) {
        *self.last_used.lock().unwrap() = Instant::now();
    }

    fn stop_engine_if_idle(&self) {
        if self.indexing.load(Ordering::SeqCst) {
            return;
        }
        let mut slot = self.engine.lock().unwrap();
        if self.last_used.lock().unwrap().elapsed() > ENGINE_IDLE {
            *slot = None;
        }
    }
}

/// `runtime/` holds the llama.cpp builds and model files. It is looked up next
/// to the executable and in its parent folders, so `tauri dev` finds the one in the repo.
fn find_runtime() -> PathBuf {
    if let Some(dir) = env_path("FILEDIG_RUNTIME") {
        return dir;
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.ancestors()
        .skip(1)
        .map(|dir| dir.join("runtime"))
        .find(|dir| dir.join("models").is_dir())
        .unwrap_or_else(|| exe.with_file_name("runtime"))
}

/// A path from the environment; a variable that is set but empty counts as unset.
fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Where the index lives when it is not the per-user app data folder: the path in
/// FILEDIG_DATA, or a `data` folder beside the executable, which is what makes a
/// copy of the app portable.
fn custom_data_dir() -> Option<PathBuf> {
    if let Some(dir) = env_path("FILEDIG_DATA") {
        return Some(dir);
    }
    let beside_exe = std::env::current_exe().ok()?.parent()?.join("data");
    beside_exe.is_dir().then_some(beside_exe)
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    format!("{e:#}")
}

#[derive(Serialize)]
struct Status {
    backend: String,
    engine_loaded: bool,
    files: i64,
    chunks: i64,
    hotkey: &'static str,
    progress: Progress,
}

#[tauri::command(async)]
fn status(state: State<AppState>) -> CmdResult<Status> {
    let conn = db::open(&state.db_path).map_err(err)?;
    let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).map_err(err);
    Ok(Status {
        backend: state.backend.lock().unwrap().clone(),
        engine_loaded: state.engine.try_lock().map(|e| e.is_some()).unwrap_or(false),
        files: count("SELECT COUNT(*) FROM files")?,
        chunks: count("SELECT COUNT(*) FROM chunks")?,
        hotkey: HOTKEY,
        progress: state.progress.lock().unwrap().clone(),
    })
}

#[derive(Serialize)]
struct Folder {
    id: i64,
    path: String,
    files: i64,
    /// Files still waiting to be indexed; None until the folder's first scan.
    pending: Option<i64>,
}

#[tauri::command(async)]
fn list_folders(state: State<AppState>) -> CmdResult<Vec<Folder>> {
    let conn = db::open(&state.db_path).map_err(err)?;
    let mut stmt = conn
        .prepare(
            "SELECT d.id, d.path, (SELECT COUNT(*) FROM files f WHERE f.folder_id = d.id), d.pending
             FROM folders d ORDER BY d.path",
        )
        .map_err(err)?;
    let rows = stmt
        .query_map([], |r| Ok(Folder { id: r.get(0)?, path: r.get(1)?, files: r.get(2)?, pending: r.get(3)? }))
        .map_err(err)?;
    rows.collect::<rusqlite::Result<_>>().map_err(err)
}

#[tauri::command(async)]
fn add_folder(app: AppHandle, state: State<AppState>, path: String) -> CmdResult<()> {
    if !std::path::Path::new(&path).is_dir() {
        return Err(format!("not a folder: {path}"));
    }
    let conn = db::open(&state.db_path).map_err(err)?;
    // Overlapping folders would make two folders claim the same files.
    if let Some(outer) = db::covering_folder(&conn, &path).map_err(err)? {
        return Err(format!("Already searched as part of {outer}"));
    }
    // Folders inside the new one are folded into it when the next pass starts.
    conn.execute("INSERT OR IGNORE INTO folders (path) VALUES (?1)", params![path]).map_err(err)?;
    indexer::start(app);
    Ok(())
}

#[tauri::command(async)]
fn remove_folder(app: AppHandle, state: State<AppState>, id: i64) -> CmdResult<()> {
    // A running pass could still be writing this folder's files: stop it first.
    let was_indexing = state.indexing.load(Ordering::SeqCst);
    if was_indexing {
        state.rescan.store(false, Ordering::SeqCst);
        state.cancel.store(true, Ordering::SeqCst);
        while state.indexing.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let conn = db::open(&state.db_path).map_err(err)?;
    db::delete_folder(&conn, id).map_err(err)?;
    state.cache_dirty.store(true, Ordering::SeqCst);
    if was_indexing {
        indexer::start(app);
    }
    Ok(())
}

#[tauri::command(async)]
fn reindex(app: AppHandle) {
    indexer::start(app);
}

/// Ends the running pass after the file in progress. Files already indexed stay searchable.
#[tauri::command]
fn stop_indexing(state: State<AppState>) {
    state.rescan.store(false, Ordering::SeqCst);
    state.cancel.store(true, Ordering::SeqCst);
}

#[derive(Serialize)]
struct Hit {
    path: String,
    name: String,
    kind: String,
    page: Option<i64>,
    snippet: String,
    /// Cosine similarity; absent for hits found by keyword only.
    score: Option<f32>,
    keyword: bool,
    /// Key of the stored preview, for images: pass it to the `thumb` protocol.
    thumb: Option<String>,
}

#[tauri::command(async)]
fn search(state: State<AppState>, query: String, filter: String, limit: usize) -> CmdResult<Vec<Hit>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let allow = |kind: u8| match filter.as_str() {
        "docs" => kind != db::KIND_IMAGE,
        "images" => kind == db::KIND_IMAGE,
        _ => true,
    };
    let conn = db::open(&state.db_path).map_err(err)?;

    let engine = state.engine().map_err(err)?;
    state.searching.fetch_add(1, Ordering::SeqCst);
    let embedded = engine.embed(&[Input::Text(format!("task: search result | query: {query}"))]);
    state.searching.fetch_sub(1, Ordering::SeqCst);
    let qvec = embedded.map_err(err)?.remove(0);

    let semantic = {
        let mut cache = state.cache.lock().unwrap();
        if state.cache_dirty.swap(false, Ordering::SeqCst) {
            *cache = VecCache::load(&conn).map_err(|e| {
                state.cache_dirty.store(true, Ordering::SeqCst);
                err(e)
            })?;
        }
        cache.top_k(&qvec, 200, allow)
    };

    // Exact-match side: every query word must appear in the passage.
    let fts_query = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{w}\""))
        .collect::<Vec<_>>()
        .join(" ");
    let mut keyword: Vec<i64> = Vec::new();
    if !fts_query.is_empty() {
        let mut stmt = conn
            .prepare(
                "SELECT c.id, f.kind FROM chunks_fts
                 JOIN chunks c ON c.id = chunks_fts.rowid
                 JOIN files f ON f.id = c.file_id
                 WHERE chunks_fts MATCH ?1 ORDER BY bm25(chunks_fts) LIMIT 100",
            )
            .map_err(err)?;
        let rows = stmt
            .query_map(params![fts_query], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(err)?;
        for row in rows {
            let (id, kind) = row.map_err(err)?;
            if allow(db::kind_code(&kind)) {
                keyword.push(id);
            }
        }
    }

    // Reciprocal rank fusion of the two rankings.
    let rrf = |rank: usize| 1.0 / (60.0 + rank as f64);
    let mut fused: HashMap<i64, (f64, Option<f32>, bool)> = HashMap::new();
    for (rank, (id, score)) in semantic.iter().enumerate() {
        fused.insert(*id, (rrf(rank), Some(*score), false));
    }
    for (rank, id) in keyword.iter().enumerate() {
        let entry = fused.entry(*id).or_insert((0.0, None, true));
        entry.0 += rrf(rank);
        entry.2 = true;
    }
    let mut ranked: Vec<_> = fused.into_iter().collect();
    ranked.sort_unstable_by(|a, b| b.1 .0.total_cmp(&a.1 .0));

    // Best passage per file.
    let mut stmt = conn
        .prepare(
            "SELECT f.path, f.kind, c.page, c.text, f.id, f.mtime FROM chunks c
             JOIN files f ON f.id = c.file_id WHERE c.id = ?1",
        )
        .map_err(err)?;
    let mut seen = HashSet::new();
    let mut hits = Vec::new();
    for (id, (_, score, keyword)) in ranked {
        if hits.len() >= limit {
            break;
        }
        let row = stmt.query_row(params![id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
            ))
        });
        // The chunk may have been removed by the indexer since the cache was loaded.
        let Ok((path, kind, page, text, file_id, mtime)) = row else { continue };
        if !seen.insert(path.clone()) {
            continue;
        }
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let snippet = if kind == "image" {
            String::new()
        } else {
            text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(260).collect()
        };
        // The modification time makes the key change when the file does, so previews can be cached.
        let thumb = (kind == "image").then(|| format!("{file_id}-{mtime}"));
        hits.push(Hit { path, name, kind, page, snippet, score, keyword, thumb });
    }
    Ok(hits)
}

#[tauri::command]
fn open_file(app: AppHandle, path: String) -> CmdResult<()> {
    app.opener().open_path(path, None::<&str>).map_err(err)
}

#[tauri::command]
fn reveal_file(app: AppHandle, path: String) -> CmdResult<()> {
    app.opener().reveal_item_in_dir(path).map_err(err)
}

fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn toggle_window(app: &AppHandle) {
    match app.get_webview_window("main") {
        Some(w) if w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) => {
            let _ = w.hide();
        }
        _ => show_window(app),
    }
}

/// Serves `thumb://<file id>-<mtime>`: the small preview stored with an indexed image.
/// Previews keep result lists light; full-size photos would be decoded on every redraw.
fn thumb_response(state: &AppState, key: &str) -> anyhow::Result<Vec<u8>> {
    let file_id: i64 = key.trim_start_matches('/').split('-').next().unwrap_or("").parse()?;
    let conn = db::open(&state.db_path)?;
    let stored: Option<Vec<u8>> = conn
        .query_row("SELECT jpeg FROM thumbs WHERE file_id = ?1", params![file_id], |r| r.get(0))
        .ok();
    if let Some(jpeg) = stored {
        return Ok(jpeg);
    }

    // Indexed before previews existed: make one now and keep it. Decoding a large
    // photo is heavy, so only a few run at once.
    static SLOTS: (Mutex<usize>, Condvar) = (Mutex::new(3), Condvar::new());
    let path: String =
        conn.query_row("SELECT path FROM files WHERE id = ?1", params![file_id], |r| r.get(0))?;
    let mut free = SLOTS.1.wait_while(SLOTS.0.lock().unwrap(), |n| *n == 0).unwrap();
    *free -= 1;
    drop(free);
    let made = indexer::make_thumb(std::path::Path::new(&path));
    *SLOTS.0.lock().unwrap() += 1;
    SLOTS.1.notify_one();

    let jpeg = made?;
    conn.execute(
        "INSERT OR REPLACE INTO thumbs (file_id, jpeg) VALUES (?1, ?2)",
        params![file_id, jpeg],
    )?;
    Ok(jpeg)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // A portable copy keeps the browser engine's profile beside the app as well.
    if let Some(dir) = custom_data_dir() {
        if env_path("WEBVIEW2_USER_DATA_FOLDER").is_none() {
            std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", dir.join("webview"));
        }
    }

    tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("thumb", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let key = request.uri().path().to_string();
            std::thread::spawn(move || {
                let response = match thumb_response(&app.state::<AppState>(), &key) {
                    Ok(jpeg) => tauri::http::Response::builder()
                        .header("Content-Type", "image/jpeg")
                        .header("Cache-Control", "max-age=31536000, immutable")
                        .body(jpeg),
                    Err(_) => tauri::http::Response::builder().status(404).body(Vec::new()),
                };
                responder.respond(response.unwrap());
            });
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        toggle_window(app);
                    }
                })
                .build(),
        )
        .setup(|app| {
            let data_dir = match custom_data_dir() {
                Some(dir) => dir,
                None => app.path().app_data_dir()?,
            };
            std::fs::create_dir_all(&data_dir)?;
            app.manage(AppState {
                db_path: data_dir.join("index.db"),
                runtime: find_runtime(),
                log_dir: data_dir,
                engine: Mutex::new(None),
                last_used: Mutex::new(Instant::now()),
                backend: Mutex::new(String::new()),
                progress: Mutex::new(Progress::default()),
                indexing: AtomicBool::new(false),
                rescan: AtomicBool::new(false),
                cancel: AtomicBool::new(false),
                cache: Mutex::new(VecCache::default()),
                cache_dirty: AtomicBool::new(true),
                searching: AtomicUsize::new(0),
            });

            // Another app may already own the hotkey; the tray icon still works.
            if let Err(e) = app.global_shortcut().register(HOTKEY) {
                eprintln!("could not register {HOTKEY}: {e}");
            }

            let show = MenuItem::with_id(app, "show", "Show FileDig", true, None::<&str>)?;
            let about = MenuItem::with_id(app, "about", "About FileDig", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("FileDig")
                .menu(&Menu::with_items(app, &[&show, &about, &quit])?)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_window(app),
                    "about" => {
                        show_window(app);
                        let _ = app.emit("show-about", ());
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_window(tray.app_handle());
                    }
                })
                .build(app)?;

            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(30));
                handle.state::<AppState>().stop_engine_if_idle();
            });

            // Pick up files that changed while the app was closed.
            indexer::start(app.handle().clone());
            Ok(())
        })
        // Closing the window keeps the app in the tray; Quit is in the tray menu.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            status,
            list_folders,
            add_folder,
            remove_folder,
            reindex,
            stop_indexing,
            search,
            open_file,
            reveal_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
