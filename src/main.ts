import "@fontsource-variable/inter";
import { getName, getVersion } from "@tauri-apps/api/app";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";

interface Hit {
  path: string;
  name: string;
  kind: "text" | "pdf" | "image";
  page: number | null;
  snippet: string;
  score: number | null;
  keyword: boolean;
  /** Key of the stored preview, for images. */
  thumb: string | null;
}

interface Folder {
  id: number;
  path: string;
  files: number;
  /** Files still waiting to be indexed; null until the folder's first scan. */
  pending: number | null;
}

interface Status {
  backend: string;
  engine_loaded: boolean;
  files: number;
  chunks: number;
  hotkey: string;
  progress: {
    running: boolean;
    phase: string;
    done: number;
    total: number;
    current: string;
    error: string | null;
    stopped: boolean;
  };
}

const svg = (body: string) =>
  `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">${body}</svg>`;

const ICONS: Record<string, string> = {
  search: svg('<circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/>'),
  close: svg('<path d="M6 6l12 12M18 6 6 18"/>'),
  layers: svg('<path d="m12 3 9 5-9 5-9-5 9-5Z"/><path d="m3 13 9 5 9-5"/>'),
  doc: svg('<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8l-5-5Z"/><path d="M14 3v5h5M9 13h6M9 17h4"/>'),
  image: svg('<rect x="3" y="4" width="18" height="16" rx="2.5"/><circle cx="8.5" cy="9.5" r="1.5"/><path d="m21 15-5-5L5 20"/>'),
  folder: svg('<path d="M3 7a2 2 0 0 1 2-2h4l2 2.5h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z"/>'),
  plus: svg('<path d="M12 5v14M5 12h14"/>'),
  refresh: svg('<path d="M20 11a8 8 0 0 0-14.3-4.3L4 8.5M4 4v4.5h4.5M4 13a8 8 0 0 0 14.3 4.3l1.7-1.8M20 20v-4.5h-4.5"/>'),
  gear: svg('<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>'),
  download: svg('<path d="M12 4v11M7.5 11 12 15.5 16.5 11M5 20h14"/>'),
  info: svg('<circle cx="12" cy="12" r="9"/><path d="M12 11v5.5M12 7.5v.3"/>'),
  warn: svg('<path d="M12 4 2.8 19.5h18.4L12 4Z"/><path d="M12 10v4.5M12 17.2v.3"/>'),
  trash: svg('<path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"/>'),
  open: svg('<path d="M14 4h6v6M20 4l-9 9M18 14v4a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4"/>'),
  lock: svg('<rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/>'),
  sheet: svg('<rect x="4" y="4" width="16" height="16" rx="2"/><path d="M4 10h16M4 15h16M10 4v16"/>'),
  code: svg('<path d="m9 8-4 4 4 4M15 8l4 4-4 4"/>'),
};

/** Example searches offered on the start screen. */
const SUGGESTIONS = ["invoice with a total amount", "photo of a sunset", "wifi password", "rental agreement"];

/** Colour family, icon and display name for each supported extension. */
const FILE_TYPES: Record<string, [type: string, icon: string, label: string]> = {
  pdf: ["pdf", "doc", "PDF"],
  docx: ["doc", "doc", "Word document"],
  csv: ["sheet", "sheet", "CSV"],
  html: ["web", "code", "Web page"],
  htm: ["web", "code", "Web page"],
  md: ["text", "doc", "Markdown"],
  markdown: ["text", "doc", "Markdown"],
  rst: ["text", "doc", "Text"],
  txt: ["text", "doc", "Text"],
  log: ["text", "doc", "Log"],
};

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const queryEl = $<HTMLInputElement>("query");
const resultsEl = $("results");
const metaEl = $("meta");
const foldersEl = $("folders");
const drawer = $("drawer");
const backdrop = $("backdrop");
const about = $<HTMLDialogElement>("about");
const settings = $<HTMLDialogElement>("settings");

let filter = "all";
let hits: Hit[] = [];
let selected = 0;
let searchSeq = 0;
let debounce: number | undefined;
let wasIndexing = false;
let folders: Folder[] = [];
let folderCount = 0;
let indexing = false;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function icon(name: string) {
  const span = el("span", "icon");
  span.innerHTML = ICONS[name];
  return span;
}

for (const node of document.querySelectorAll<HTMLElement>("[data-icon]")) {
  node.innerHTML = ICONS[node.dataset.icon!];
}

function emptyState(iconName: string, title: string, detail: string, ...extra: HTMLElement[]) {
  const box = el("div", "empty");
  const badge = el("div", "empty-icon");
  badge.append(icon(iconName));
  box.append(badge, el("h3", undefined, title), el("p", undefined, detail), ...extra);
  resultsEl.className = "";
  resultsEl.replaceChildren(box);
  metaEl.textContent = "";
}

let toastTimer: number | undefined;

/** Brief message at the bottom of the window; results stay on screen. */
function toast(text: string, isError = false) {
  const node = $("toast");
  node.textContent = text;
  node.className = "show" + (isError ? " error" : "");
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (node.className = ""), 3500);
}

function showIdle() {
  hits = [];
  if (setup && !setup.model_installed) {
    const get = el("button", "accent", `Download model (${megabytes(setup.model_size)})`);
    get.prepend(icon("download"));
    get.addEventListener("click", () => {
      openSettings();
      startDownload("model");
    });
    emptyState(
      "download",
      "One download to get started",
      "FileDig needs its search model. It is downloaded once and then runs entirely on this computer.",
      get,
    );
    return;
  }
  if (!folderCount) {
    const add = el("button", "accent", "Add folder");
    add.prepend(icon("plus"));
    add.addEventListener("click", addFolder);
    emptyState(
      "folder",
      "Add a folder to search",
      "FileDig reads the documents and pictures in the folders you choose, so you can find them by what they contain.",
      add,
      el("span", "hint", "You can also drag a folder here."),
    );
    return;
  }
  const chips = el("div", "suggestions");
  for (const text of SUGGESTIONS) {
    const chip = el("button", undefined, text);
    chip.prepend(icon("search"));
    chip.addEventListener("click", () => {
      queryEl.value = text;
      queryEl.focus();
      onQueryChanged();
    });
    chips.append(chip);
  }
  emptyState(
    "search",
    "Search your files",
    "Find documents and pictures by what they contain, or by an exact name or number.",
    chips,
  );
}

async function runSearch() {
  const query = queryEl.value.trim();
  const seq = ++searchSeq;
  if (!query) {
    $("spinner").hidden = true;
    showIdle();
    return;
  }
  $("spinner").hidden = false;
  const started = performance.now();
  try {
    const found = await invoke<Hit[]>("search", { query, filter, limit: 48 });
    if (seq !== searchSeq) return;
    hits = found;
    selected = 0;
    renderHits();
    const ms = Math.round(performance.now() - started);
    metaEl.textContent = hits.length ? `${hits.length} result${hits.length === 1 ? "" : "s"} · ${ms} ms` : "";
  } catch (e) {
    if (seq === searchSeq) emptyState("close", "Search failed", String(e));
  } finally {
    if (seq === searchSeq) $("spinner").hidden = true;
  }
}

/** Wraps the query's words in <mark> wherever they occur in `text`. */
function highlighted(text: string) {
  const node = el("div", "snippet");
  const words = queryEl.value.split(/[^\p{L}\p{N}]+/u).filter((w) => w.length > 1);
  if (!words.length) {
    node.textContent = text;
    return node;
  }
  const pattern = new RegExp(`(${words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})`, "giu");
  text.split(pattern).forEach((part, i) => node.append(i % 2 ? el("mark", undefined, part) : part));
  return node;
}

/**
 * How close a result is, in words. "Exact match" means every query word occurs
 * in the file; the other grades come from the similarity score.
 */
function matchLabel(hit: Hit) {
  const label = el("span", "match");
  if (hit.keyword) {
    label.classList.add("strong");
    label.textContent = "Exact match";
  } else if (hit.score !== null) {
    // Cosine scores for this model sit between roughly 0.45 (unrelated) and 0.8 (very close).
    const strength = (hit.score - 0.45) / 0.35;
    if (strength >= 0.7) label.classList.add("strong");
    label.textContent = strength >= 0.7 ? "Strong match" : strength >= 0.4 ? "Good match" : "Related";
  }
  if (hit.score !== null) label.title = `Similarity ${hit.score.toFixed(2)}`;
  return label;
}

function extension(name: string) {
  const dot = name.lastIndexOf(".");
  return dot < 0 ? "" : name.slice(dot + 1).toLowerCase();
}

/** The containing folder, shortened: "Downloads › Bills" instead of the full path. */
function locationOf(path: string) {
  const dir = path.slice(0, Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/")));
  // Drop the home directory prefix on Windows, Linux and macOS alike.
  const short = dir.replace(/^(?:[A-Za-z]:\\Users\\[^\\]+\\?|\/(?:home|Users)\/[^/]+\/?)/, "");
  return (short || dir).split(/[\\/]/).filter(Boolean).join(" › ");
}

/** Small icon button on a result; does not trigger the row's own click. */
function action(iconName: string, title: string, run: () => void) {
  const button = el("button", "subtle");
  button.title = title;
  button.append(icon(iconName));
  button.addEventListener("click", (e) => {
    e.stopPropagation();
    run();
  });
  return button;
}

function renderHits() {
  if (!hits.length) {
    emptyState("search", "No results", "Try different words, or another filter.");
    return;
  }
  const grid = filter === "images";
  resultsEl.className = grid ? "grid" : "list";

  // Rows are reused between searches while the same file stays in the results, so
  // typing does not rebuild the list and reload every picture on each keystroke.
  const next = new Map<string, HTMLElement>();
  const rows = hits.map((hit) => {
    const key = [grid, hit.path, hit.page, hit.thumb, hit.snippet].join("\n");
    let row = rowCache.get(key);
    if (row) {
      row.classList.remove("selected");
      // Only the parts that depend on the query text need refreshing.
      row.querySelector(".snippet")?.replaceWith(highlighted(hit.snippet));
      row.querySelector(".match")?.replaceWith(matchLabel(hit));
    } else {
      row = buildRow(hit, grid);
    }
    next.set(key, row);
    return row;
  });
  rowCache = next;
  resultsEl.replaceChildren(...rows);
  select(selected);
}

let rowCache = new Map<string, HTMLElement>();

function buildRow(hit: Hit, grid: boolean) {
  const row = el("div", grid ? "tile" : "hit");

  const ext = extension(hit.name);
  let kindLabel = `${ext.toUpperCase()} image`;
  if (hit.kind === "image") {
    const img = el("img", "thumb");
    img.loading = "lazy";
    img.decoding = "async";
    // A small stored preview, not the original file.
    img.src = hit.thumb ? convertFileSrc(hit.thumb, "thumb") : convertFileSrc(hit.path);
    row.append(img);
  } else {
    const [type, iconName, label] = FILE_TYPES[ext] ?? ["text", "doc", "Text"];
    kindLabel = label;
    const badge = el("div", "thumb filetype");
    badge.dataset.type = type;
    badge.append(icon(iconName));
    row.append(badge);
  }

  const body = el("div", "body");
  body.append(el("div", "name", hit.name));
  if (hit.snippet && !grid) body.append(highlighted(hit.snippet));
  if (!grid) {
    // Kind · page · where it lives, like the detail line in a file manager.
    const details = [kindLabel, hit.page ? `Page ${hit.page}` : "", locationOf(hit.path)];
    body.append(el("div", "path", details.filter(Boolean).join("  ·  ")));
  }
  row.append(body, matchLabel(hit));
  if (!grid) {
    const actions = el("div", "actions");
    actions.append(
      action("open", "Open", () => openHit(hit, false)),
      action("folder", "Show in folder", () => openHit(hit, true)),
    );
    row.append(actions);
  }
  row.title = hit.path;

  row.addEventListener("click", (e) => {
    // The row's position changes as results update, so look it up when clicked.
    select(Array.prototype.indexOf.call(resultsEl.children, row));
    openHit(hit, e.ctrlKey);
  });
  return row;
}

function select(index: number) {
  selected = index;
  resultsEl.querySelector(".selected")?.classList.remove("selected");
  const row = resultsEl.children[index];
  row?.classList.add("selected");
  row?.scrollIntoView({ block: "nearest" });
}

function openHit(hit: Hit, reveal: boolean) {
  invoke(reveal ? "reveal_file" : "open_file", { path: hit.path }).catch((e) =>
    toast(`Could not open ${hit.name}: ${e}`, true),
  );
}

/** The line under a folder's path: how much of it is searchable. */
function folderState(folder: Folder) {
  const files = `${folder.files} file${folder.files === 1 ? "" : "s"}`;
  if (folder.pending === null) return el("div", "count warn", indexing ? "Waiting to be indexed" : "Not indexed yet");
  if (folder.pending > 0) {
    return el("div", "count warn", `${files} indexed · ${folder.pending} ${indexing ? "to go" : "not indexed"}`);
  }
  return el("div", "count", files);
}

/** Tells the user when search cannot see everything yet, with the way out. */
function updateBanner() {
  const banner = $("banner");
  const action = $("banner-action");
  const incomplete = folders.filter((f) => f.pending !== 0);
  banner.hidden = !indexing && !incomplete.length;
  banner.classList.toggle("warn", !indexing);
  banner.querySelector(".icon")!.innerHTML = ICONS[indexing ? "info" : "warn"];
  if (indexing) {
    $("banner-text").textContent = "Indexing your files. Some results may be missing until it finishes.";
    action.textContent = "Stop";
  } else if (incomplete.length) {
    const names = incomplete.map((f) => f.path.split(/[\\/]/).filter(Boolean).pop()).join(", ");
    $("banner-text").textContent =
      `Indexing is not finished for ${names}. Files that have not been indexed will not appear.`;
    action.textContent = "Resume";
  }
}

async function refreshFolders() {
  const hadFolders = folderCount > 0;
  folders = await invoke<Folder[]>("list_folders");
  folderCount = folders.length;
  $("folder-count").hidden = !folderCount;
  $("folder-count").textContent = String(folderCount);
  if (!folders.length) {
    foldersEl.replaceChildren(el("li", "none", "No folders yet. Add one to start searching."));
  } else {
    foldersEl.replaceChildren(
      ...folders.map((folder) => {
        const li = el("li");
        const info = el("div", "folder-info");
        info.append(
          el("div", "folder-name", folder.path.split(/[\\/]/).filter(Boolean).pop() ?? folder.path),
          el("div", "path", folder.path),
          folderState(folder),
        );
        const remove = el("button", "subtle danger");
        remove.title = "Stop searching this folder";
        remove.append(icon("trash"));
        remove.addEventListener("click", async () => {
          await invoke("remove_folder", { id: folder.id });
          await refreshFolders();
          runSearch();
        });
        li.append(icon("folder"), info, remove);
        return li;
      }),
    );
  }
  updateBanner();
  if (hadFolders !== folderCount > 0 && !queryEl.value.trim()) showIdle();
}

async function refreshStatus() {
  const s = await invoke<Status>("status");
  const p = s.progress;
  const statusEl = $("status");
  indexing = p.running;
  document.body.classList.toggle("indexing", p.running);
  $("progress-fill").style.width = p.running && p.total ? `${(p.done / p.total) * 100}%` : "0";

  if (p.running) {
    const file = p.current.split(/[\\/]/).pop();
    statusEl.textContent = (p.total ? `${p.phase} ${p.done} of ${p.total}` : p.phase) + (file ? ` · ${file}` : "");
  } else if (p.error) {
    statusEl.textContent = `Indexing error: ${p.error}`;
  } else {
    statusEl.textContent = `${s.files.toLocaleString()} files indexed`;
  }

  const backend = $("backend");
  backend.hidden = !s.backend;
  backend.textContent = s.backend === "CUDA" ? "GPU" : s.backend;
  backend.classList.toggle("on", s.engine_loaded);
  backend.title = s.engine_loaded ? `Model loaded on ${s.backend}` : "Model unloaded; it reloads on the next search";

  // Folder counts while a pass runs, and fresh results once it finishes.
  if (p.running || wasIndexing) await refreshFolders();
  else updateBanner();
  if (wasIndexing && !p.running) runSearch();
  wasIndexing = p.running;
}

async function addFolder() {
  const path = await open({ directory: true, title: "Choose a folder to search" });
  if (typeof path === "string") addFolders([path]);
}

/** Adds each path that is a folder and starts indexing; anything else is reported. */
async function addFolders(paths: string[]) {
  const added: string[] = [];
  let problem = "";
  for (const path of paths) {
    try {
      await invoke("add_folder", { path });
      added.push(path.split(/[\\/]/).filter(Boolean).pop() ?? path);
    } catch (e) {
      // Not a folder, or already covered by a folder in the list.
      problem = String(e).startsWith("not a folder") ? "Only folders can be added." : String(e);
    }
  }
  if (added.length) toast(`Indexing ${added.join(", ")}. You can search while it finishes.`);
  else toast(problem, true);
  await refreshFolders();
  refreshStatus();
}

function setDrawer(openState: boolean) {
  drawer.classList.toggle("open", openState);
  drawer.setAttribute("aria-hidden", String(!openState));
  backdrop.hidden = !openState;
}

function onQueryChanged() {
  $("clear").hidden = !queryEl.value;
  clearTimeout(debounce);
  debounce = window.setTimeout(runSearch, 180);
}

queryEl.addEventListener("input", onQueryChanged);

$("clear").addEventListener("click", () => {
  queryEl.value = "";
  queryEl.focus();
  onQueryChanged();
});

window.addEventListener("keydown", (e) => {
  // The dialog handles its own keys, including Esc to close.
  if (about.open || settings.open) return;
  if (e.key === "Escape") {
    if (drawer.classList.contains("open")) setDrawer(false);
    else getCurrentWindow().hide();
    return;
  }
  if (!hits.length || drawer.classList.contains("open")) return;

  const grid = filter === "images";
  const columns = grid ? getComputedStyle(resultsEl).gridTemplateColumns.split(" ").length : 1;
  const step: Record<string, number> = { ArrowDown: columns, ArrowUp: -columns };
  if (grid) Object.assign(step, { ArrowRight: 1, ArrowLeft: -1 });

  if (e.key in step) {
    const next = selected + step[e.key];
    if (next >= 0 && next < hits.length) select(next);
    e.preventDefault();
  } else if (e.key === "Enter") {
    openHit(hits[selected], e.ctrlKey);
  }
});

$("filters").addEventListener("click", (e) => {
  const button = (e.target as HTMLElement).closest("button");
  if (!button) return;
  filter = button.dataset.filter ?? "all";
  for (const b of $("filters").querySelectorAll("button")) b.classList.toggle("active", b === button);
  queryEl.focus();
  runSearch();
});

$("folders-toggle").addEventListener("click", () => setDrawer(true));
$("drawer-close").addEventListener("click", () => setDrawer(false));
backdrop.addEventListener("click", () => setDrawer(false));
$("add-folder").addEventListener("click", addFolder);
$("reindex").addEventListener("click", async () => {
  await invoke("reindex");
  refreshStatus();
});

$("banner-action").addEventListener("click", async () => {
  await invoke(indexing ? "stop_indexing" : "reindex");
  refreshStatus();
});

/** Fills the About dialog with the version and current state, then opens it. */
async function showAbout() {
  const [name, version, s] = await Promise.all([getName(), getVersion(), invoke<Status>("status")]);
  $("about-name").textContent = name;
  $("about-version").textContent = `Version ${version}`;
  const device = s.backend === "CUDA" ? "GPU (NVIDIA CUDA)" : s.backend;
  $("about-engine").textContent = !s.backend
    ? "Not started yet; it loads on the first search"
    : device + (s.engine_loaded ? "" : ", unloaded while idle");
  $("about-index").textContent =
    `${s.files.toLocaleString()} file${s.files === 1 ? "" : "s"} in ${folderCount} folder${folderCount === 1 ? "" : "s"}`;
  if (!about.open) about.showModal();
}

$("about-open").addEventListener("click", showAbout);
$("about-close").addEventListener("click", () => about.close());
// A click on the dimmed area around the dialog closes it.
about.addEventListener("click", (e) => {
  if (e.target === about) about.close();
});
listen("show-about", showAbout);

interface Setup {
  model_installed: boolean;
  model_size: number;
  gpu_installed: boolean;
  gpu_supported: boolean;
  gpu_size: number;
  download: {
    component: string;
    running: boolean;
    done: number;
    total: number;
    step: string;
    error: string | null;
    cancelled: boolean;
  };
}

let setup: Setup | null = null;
let wasDownloading = false;

const megabytes = (bytes: number) => `${Math.round(bytes / 1048576).toLocaleString()} MB`;

/** Reads what is installed and any download in flight, and reflects it in Settings. */
async function refreshSetup() {
  const hadModel = setup?.model_installed;
  setup = await invoke<Setup>("setup_status");
  const d = setup.download;

  $("gpu-state").textContent = setup.gpu_installed
    ? "Installed. Searching and indexing use your NVIDIA graphics card."
    : setup.gpu_supported
      ? "An NVIDIA graphics card was found. Downloading GPU support makes indexing several times faster."
      : "No NVIDIA graphics card was found, so FileDig runs on the processor.";
  const gpuAction = $<HTMLButtonElement>("gpu-action");
  gpuAction.hidden = setup.gpu_installed || !setup.gpu_supported;
  gpuAction.textContent = `Download GPU support (${megabytes(setup.gpu_size)})`;
  gpuAction.disabled = d.running;

  $("model-state").textContent = setup.model_installed
    ? "Installed: EmbeddingGemma 2 (8-bit)."
    : "Not installed. FileDig cannot search until the model is downloaded.";
  const modelAction = $<HTMLButtonElement>("model-action");
  modelAction.hidden = setup.model_installed;
  modelAction.textContent = `Download model (${megabytes(setup.model_size)})`;
  modelAction.disabled = d.running;

  $("download").hidden = !d.running;
  if (d.running) {
    $("download-fill").style.width = `${d.total ? (d.done / d.total) * 100 : 0}%`;
    $("download-text").textContent = `${d.step} · ${megabytes(d.done)} of ${megabytes(d.total)}`;
  }
  const failed = $("download-error");
  failed.hidden = d.running || !d.error;
  failed.textContent = d.error ? `Download failed: ${d.error}. Try again to continue where it stopped.` : "";

  if (wasDownloading && !d.running && !d.error && !d.cancelled) {
    toast(d.component === "gpu" ? "GPU support is installed." : "The model is installed.");
    // Folders added before the model arrived can be indexed now.
    if (d.component === "model") invoke("reindex");
    refreshStatus();
  }
  wasDownloading = d.running;
  if (hadModel !== setup.model_installed && !queryEl.value.trim()) showIdle();
}

async function startDownload(component: "gpu" | "model") {
  try {
    await invoke("download_component", { component });
  } catch (e) {
    toast(String(e), true);
  }
  refreshSetup();
}

function openSettings() {
  refreshSetup();
  if (!settings.open) settings.showModal();
}

$("settings-open").addEventListener("click", openSettings);
$("settings-close").addEventListener("click", () => settings.close());
$("gpu-action").addEventListener("click", () => startDownload("gpu"));
$("model-action").addEventListener("click", () => startDownload("model"));
$("download-cancel").addEventListener("click", async () => {
  await invoke("cancel_download");
  refreshSetup();
});
// Keep the progress moving while a download runs or the dialog is open.
setInterval(() => {
  if (settings.open || setup?.download.running) refreshSetup();
}, 700);

getCurrentWebview().onDragDropEvent(({ payload }) => {
  $("drop").hidden = payload.type !== "enter" && payload.type !== "over";
  if (payload.type === "drop") addFolders(payload.paths);
});

getCurrentWindow().onFocusChanged(({ payload: focused }) => {
  if (focused) {
    queryEl.focus();
    queryEl.select();
  }
});

refreshSetup().then(refreshFolders).then(showIdle);
refreshStatus();
setInterval(refreshStatus, 1500);
