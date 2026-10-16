mod engine;
mod indexer;
mod ocr;
mod setup;
mod watcher;

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

use nlsearch_core::db::{self, Settings, VecCache};
use nlsearch_core::embed::Embedder;
use nlsearch_core::search::{self, Filters, Hit};

use engine::Engine;

const RELEASES: &str = "https://github.com/jason7773/NLsearch/releases/latest";
const HOTKEY: &str = "ctrl+shift+space";
/// Passed when Windows starts NLsearch at sign-in, so it opens in the tray only.
const HIDDEN_ARG: &str = "--hidden";
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
    /// Folders holding the model and llama.cpp builds: downloaded parts first (this
    /// one is writable), then what shipped with the app.
    pub runtimes: Vec<PathBuf>,
    pub download: Mutex<setup::Download>,
    pub download_cancel: AtomicBool,
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
    /// Watches the indexed folders; replaced when they change.
    pub watcher: Mutex<Option<notify::RecommendedWatcher>>,
    /// Tells the watcher thread that something changed.
    pub watch_sender: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

impl AppState {
    /// Returns the running engine, starting it first if needed.
    pub fn engine(&self) -> anyhow::Result<Arc<Engine>> {
        let mut slot = self.engine.lock().unwrap();
        self.touch();
        if let Some(engine) = slot.as_ref() {
            return Ok(engine.clone());
        }
        let engine = Arc::new(Engine::start(&self.runtimes, &self.log_dir)?);
        *self.backend.lock().unwrap() = engine.backend.to_string();
        *slot = Some(engine.clone());
        Ok(engine)
    }

    /// Stops the helper so the next use starts it afresh, picking up a new backend.
    pub fn restart_engine(&self) {
        *self.engine.lock().unwrap() = None;
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
    if let Some(dir) = env_path("NLSEARCH_RUNTIME") {
        return dir;
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.ancestors()
        .skip(1)
        .map(|dir| dir.join("runtime"))
        .find(|dir| dir.is_dir())
        .unwrap_or_else(|| exe.with_file_name("runtime"))
}

/// A path from the environment; a variable that is set but empty counts as unset.
fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Where the index lives when it is not the per-user app data folder: the path in
/// NLSEARCH_DATA, or a `data` folder beside the executable, which is what makes a
/// copy of the app portable.
fn custom_data_dir() -> Option<PathBuf> {
    if let Some(dir) = env_path("NLSEARCH_DATA") {
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
struct Setup {
    model_installed: bool,
    model_size: u64,
    gpu_installed: bool,
    /// An NVIDIA driver is present, so the GPU backend would work here.
    gpu_supported: bool,
    gpu_size: u64,
    download: setup::Download,
}

#[tauri::command(async)]
fn setup_status(state: State<AppState>) -> Setup {
    Setup {
        model_installed: engine::model_installed(&state.runtimes),
        model_size: setup::download_size("model"),
        gpu_installed: engine::gpu_backend_installed(&state.runtimes),
        gpu_supported: setup::nvidia_present(),
        gpu_size: setup::download_size("gpu"),
        download: state.download.lock().unwrap().clone(),
    }
}

#[tauri::command]
fn download_component(app: AppHandle, component: String) -> CmdResult<()> {
    setup::start(app, component)
}

#[tauri::command]
fn cancel_download(state: State<AppState>) {
    state.download_cancel.store(true, Ordering::SeqCst);
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
    watcher::refresh(&app);
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
    watcher::refresh(&app);
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
struct SettingsView {
    #[serde(flatten)]
    settings: Settings,
    /// Languages Windows can read with OCR; empty when it cannot.
    ocr_languages: Vec<String>,
    /// OCR works with the languages of the user's Windows profile.
    ocr_available: bool,
}

#[tauri::command(async)]
fn get_settings(state: State<AppState>) -> CmdResult<SettingsView> {
    let conn = db::open(&state.db_path).map_err(err)?;
    Ok(SettingsView {
        settings: Settings::load(&conn).map_err(err)?,
        ocr_languages: ocr::languages(),
        ocr_available: ocr::engine().is_some(),
    })
}

/// Saves the settings and indexes again: excluded files drop out, files no longer
/// excluded come in, and pictures and PDFs are read again when OCR changes.
#[tauri::command(async)]
fn save_settings(app: AppHandle, state: State<AppState>, settings: Settings) -> CmdResult<()> {
    let conn = db::open(&state.db_path).map_err(err)?;
    settings.save(&conn).map_err(err)?;
    watcher::refresh(&app);
    indexer::start(app);
    Ok(())
}

#[tauri::command(async)]
fn search(
    state: State<AppState>,
    query: String,
    filter: String,
    filters: Option<Filters>,
    limit: usize,
) -> CmdResult<Vec<Hit>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let conn = db::open(&state.db_path).map_err(err)?;
    let allowed = search::allowed_files(&conn, &filters.unwrap_or_default()).map_err(err)?;

    let engine = state.engine().map_err(err)?;
    state.searching.fetch_add(1, Ordering::SeqCst);
    let embedded = engine.embed(&[search::query_input(query)]);
    state.searching.fetch_sub(1, Ordering::SeqCst);
    let qvec = embedded.map_err(err)?.remove(0);

    let semantic = {
        let cache = loaded_cache(&state, &conn)?;
        cache.top_k(&qvec, search::candidates(limit), |kind, file| {
            search::allows(&filter, kind) && allowed.as_ref().map_or(true, |a| a.contains(&file))
        })
    };
    search::rank(&conn, &semantic, query, &filter, allowed.as_ref(), limit).map_err(err)
}

/// The vector cache, read again from the index first if it changed.
fn loaded_cache<'a>(state: &'a AppState, conn: &rusqlite::Connection) -> CmdResult<std::sync::MutexGuard<'a, VecCache>> {
    let mut cache = state.cache.lock().unwrap();
    if state.cache_dirty.swap(false, Ordering::SeqCst) {
        *cache = VecCache::load(conn).map_err(|e| {
            state.cache_dirty.store(true, Ordering::SeqCst);
            err(e)
        })?;
    }
    Ok(cache)
}

/// Files with content like the one at `path`. Needs no model: the file's own vectors are the query.
#[tauri::command(async)]
fn similar(
    state: State<AppState>,
    path: String,
    filter: String,
    filters: Option<Filters>,
    limit: usize,
) -> CmdResult<Vec<Hit>> {
    let conn = db::open(&state.db_path).map_err(err)?;
    let allowed = search::allowed_files(&conn, &filters.unwrap_or_default()).map_err(err)?;
    let cache = loaded_cache(&state, &conn)?;
    search::similar(&conn, &cache, &path, &filter, allowed.as_ref(), limit).map_err(err)
}

/// The indexed text of one file, for the preview pane.
#[tauri::command(async)]
fn preview(state: State<AppState>, path: String) -> CmdResult<search::Preview> {
    let conn = db::open(&state.db_path).map_err(err)?;
    search::preview(&conn, &path).map_err(err)
}

/// The tray menu's items, kept so the window can put them in the user's language.
struct TrayItems {
    show: MenuItem<tauri::Wry>,
    about: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}

#[tauri::command]
fn set_tray_labels(items: State<TrayItems>, show: String, about: String, quit: String) -> CmdResult<()> {
    items.show.set_text(show).map_err(err)?;
    items.about.set_text(about).map_err(err)?;
    items.quit.set_text(quit).map_err(err)
}

/// A portable copy cannot install updates over itself; it only points to the download.
#[tauri::command]
fn is_portable() -> bool {
    custom_data_dir().is_some()
}

#[tauri::command]
fn open_downloads(app: AppHandle) -> CmdResult<()> {
    app.opener().open_url(RELEASES, None::<&str>).map_err(err)
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
    let made = nlsearch_core::extract::make_thumb(std::path::Path::new(&path));
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

    let mut builder = tauri::Builder::default();
    // Starting NLsearch again brings the running copy to the front instead of opening a
    // second one. NLSEARCH_MULTI_INSTANCE lifts this, for testing copies side by side.
    // This plugin has to be registered before any other.
    if env_path("NLSEARCH_MULTI_INSTANCE").is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_window(app);
        }));
    }

    builder
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
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![HIDDEN_ARG]),
        ))
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
                runtimes: vec![data_dir.join("runtime"), find_runtime()],
                download: Mutex::new(setup::Download::default()),
                download_cancel: AtomicBool::new(false),
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
                watcher: Mutex::new(None),
                watch_sender: Mutex::new(None),
            });

            // The window starts hidden (see tauri.conf.json) so a start at sign-in stays in the tray.
            if !std::env::args().any(|a| a == HIDDEN_ARG) {
                show_window(app.handle());
            }

            // Another app may already own the hotkey; the tray icon still works.
            if let Err(e) = app.global_shortcut().register(HOTKEY) {
                eprintln!("could not register {HOTKEY}: {e}");
            }

            let show = MenuItem::with_id(app, "show", "Show NLsearch", true, None::<&str>)?;
            let about = MenuItem::with_id(app, "about", "About NLsearch", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("NLsearch")
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
            app.manage(TrayItems { show, about, quit });

            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(30));
                handle.state::<AppState>().stop_engine_if_idle();
            });

            // Pick up files that changed while the app was closed, then follow changes.
            indexer::start(app.handle().clone());
            watcher::start(app.handle().clone());
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
            setup_status,
            download_component,
            cancel_download,
            search,
            similar,
            preview,
            set_tray_labels,
            open_file,
            reveal_file,
            is_portable,
            open_downloads,
            get_settings,
            save_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
