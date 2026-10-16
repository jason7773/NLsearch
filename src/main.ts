import "@fontsource-variable/inter";
import "@fontsource-variable/noto-serif-tc";
import { getName, getVersion } from "@tauri-apps/api/app";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { disable as disableAutostart, enable as enableAutostart, isEnabled as autostartEnabled } from "@tauri-apps/plugin-autostart";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { open } from "@tauri-apps/plugin-dialog";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { has, lang, languageChoice, setLanguageChoice, t, tn, translatePage, type LanguageChoice, type TextKey } from "./i18n";

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
  /** Last modified, in seconds since 1970. */
  modified: number;
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

interface Preview {
  kind: Hit["kind"];
  sections: { page: number | null; text: string }[];
  truncated: boolean;
}

const svg = (body: string) =>
  `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">${body}</svg>`;

const ICONS: Record<string, string> = {
  search: svg('<circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/>'),
  close: svg('<path d="M6 6l12 12M18 6 6 18"/>'),
  doc: svg('<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8l-5-5Z"/><path d="M14 3v5h5M9 13h6M9 17h4"/>'),
  image: svg('<rect x="3" y="4" width="18" height="16" rx="2.5"/><circle cx="8.5" cy="9.5" r="1.5"/><path d="m21 15-5-5L5 20"/>'),
  folder: svg('<path d="M3 7a2 2 0 0 1 2-2h4l2 2.5h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z"/>'),
  plus: svg('<path d="M12 5v14M5 12h14"/>'),
  refresh: svg('<path d="M20 11a8 8 0 0 0-14.3-4.3L4 8.5M4 4v4.5h4.5M4 13a8 8 0 0 0 14.3 4.3l1.7-1.8M20 20v-4.5h-4.5"/>'),
  gear: svg('<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>'),
  download: svg('<path d="M12 4v11M7.5 11 12 15.5 16.5 11M5 20h14"/>'),
  bolt: svg('<path d="M13 3 5 13.5h6L10 21l8-10.5h-6L13 3Z"/>'),
  info: svg('<circle cx="12" cy="12" r="9"/><path d="M12 11v5.5M12 7.5v.3"/>'),
  warn: svg('<path d="M12 4 2.8 19.5h18.4L12 4Z"/><path d="M12 10v4.5M12 17.2v.3"/>'),
  trash: svg('<path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"/>'),
  open: svg('<path d="M14 4h6v6M20 4l-9 9M18 14v4a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4"/>'),
  sheet: svg('<rect x="4" y="4" width="16" height="16" rx="2"/><path d="M4 10h16M4 15h16M10 4v16"/>'),
  code: svg('<path d="m9 8-4 4 4 4M15 8l4 4-4 4"/>'),
  slides: svg('<rect x="3" y="4" width="18" height="13" rx="2"/><path d="M12 17v3M8 20h8"/>'),
  filter: svg('<path d="M4 5h16l-6 7.5V19l-4-2v-4.5L4 5Z"/>'),
  panel: svg('<rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M14.5 4v16"/>'),
  sparkle: svg('<path d="M12 3.5 13.8 9a2 2 0 0 0 1.2 1.2l5.5 1.8-5.5 1.8a2 2 0 0 0-1.2 1.2L12 20.5 10.2 15a2 2 0 0 0-1.2-1.2L3.5 12 9 10.2A2 2 0 0 0 10.2 9L12 3.5Z"/>'),
  copy: svg('<rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/>'),
  link: svg('<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>'),
  clock: svg('<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>'),
};

/** Example searches offered on the start screen. */
const SUGGESTIONS: TextKey[] = ["suggest.1", "suggest.2", "suggest.3", "suggest.4"];

/** Colour family, icon and display name for each supported extension. */
const FILE_TYPES: Record<string, [type: string, icon: string, label: TextKey]> = {
  pdf: ["pdf", "doc", "kind.pdf"],
  docx: ["doc", "doc", "kind.word"],
  csv: ["sheet", "sheet", "kind.csv"],
  html: ["web", "code", "kind.web"],
  htm: ["web", "code", "kind.web"],
  md: ["text", "doc", "kind.markdown"],
  markdown: ["text", "doc", "kind.markdown"],
  rst: ["text", "doc", "kind.text"],
  txt: ["text", "doc", "kind.text"],
  log: ["text", "doc", "kind.log"],
  xlsx: ["sheet", "sheet", "kind.excel"],
  pptx: ["slides", "slides", "kind.powerpoint"],
};

/** Source code and configuration files, which are indexed as text. */
const CODE_EXTENSIONS = (
  "py js mjs cjs ts tsx jsx java kt kts scala c h cpp cc hpp cs go rs rb php swift dart lua pl r sql sh ps1 bat " +
  "cmd json yaml yml toml xml ini cfg conf css scss vue svelte ipynb"
).split(" ");
for (const ext of CODE_EXTENSIONS) FILE_TYPES[ext] ??= ["code", "code", "kind.code"];

/** The file type filter's choices, and the extensions each one covers. */
const TYPE_GROUPS: [label: TextKey, extensions: string[]][] = [
  ["type.pdf", ["pdf"]],
  ["type.word", ["docx"]],
  ["type.excel", ["xlsx", "csv"]],
  ["type.powerpoint", ["pptx"]],
  ["type.text", ["txt", "md", "markdown", "rst", "log", "html", "htm"]],
  ["type.code", CODE_EXTENSIONS],
  ["type.pictures", ["jpg", "jpeg", "png", "webp", "bmp", "gif"]],
];

/** Results asked for at a time; "Show more" asks for this many more, up to MAX_RESULTS. */
const PAGE = 48;
const MAX_RESULTS = 480;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const queryEl = $<HTMLInputElement>("query");
const resultsEl = $("results");
const metaEl = $("meta");
const foldersEl = $("folders");
const drawer = $("drawer");
const backdrop = $("backdrop");
const about = $<HTMLDialogElement>("about");
const settings = $<HTMLDialogElement>("settings");
const sortEl = $<HTMLSelectElement>("sort");

let filter = "all";
/** Results as the backend ranked them; `hits` is the same list in the chosen order. */
let ranked: Hit[] = [];
let hits: Hit[] = [];
let selected = 0;
let searchSeq = 0;
let limit = PAGE;
let debounce: number | undefined;
let wasIndexing = false;
let folders: Folder[] = [];
let folderCount = 0;
let indexing = false;
/** Set while the list shows files like this one instead of a search. */
let similarTo: Hit | null = null;

translatePage();

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

/** Reads a stored value; storage can be missing or full, and then the default applies. */
function stored<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw === null ? fallback : (JSON.parse(raw) as T);
  } catch {
    return fallback;
  }
}

function store(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Kept for this session only.
  }
}

const megabytes = (bytes: number) => t("megabytes", { n: Math.round(bytes / 1048576) });
const folderName = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;

function emptyState(iconName: string, title: string, detail: string, ...extra: HTMLElement[]) {
  const box = el("div", "empty");
  const badge = el("div", "empty-icon");
  badge.append(icon(iconName));
  box.append(badge, el("h3", undefined, title), el("p", undefined, detail), ...extra);
  resultsEl.className = "panel";
  resultsEl.replaceChildren(box);
  // With nothing to preview, the start screen and messages take the whole width.
  document.body.classList.add("no-results");
  metaEl.textContent = "";
  showPreview(null);
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

async function copy(text: string, done: string) {
  try {
    await writeText(text);
    toast(done);
  } catch (e) {
    toast(t("copy.failed", { error: String(e) }), true);
  }
}

// ---------------------------------------------------------------------------
// History: recent searches and opened files, on this computer only.

interface Recent {
  path: string;
  name: string;
  kind: Hit["kind"];
  page: number | null;
}

const HISTORY_SIZE = 8;

const keepHistory = () => stored("keepHistory", true);
const recentSearches = () => stored<string[]>("recentSearches", []);
const recentFiles = () => stored<Recent[]>("recentFiles", []);

function rememberSearch(query: string) {
  if (!keepHistory() || !query) return;
  const list = recentSearches().filter((q) => q.toLowerCase() !== query.toLowerCase());
  store("recentSearches", [query, ...list].slice(0, HISTORY_SIZE));
}

function rememberFile(hit: Hit) {
  if (!keepHistory()) return;
  const list = recentFiles().filter((f) => f.path !== hit.path);
  const entry: Recent = { path: hit.path, name: hit.name, kind: hit.kind, page: hit.page };
  store("recentFiles", [entry, ...list].slice(0, HISTORY_SIZE));
}

/** A search counts as made once its results have stayed up a moment, or one is opened. */
let rememberTimer: number | undefined;

function forget(key: "recentSearches" | "recentFiles", keep: (item: never) => boolean) {
  store(key, (stored<never[]>(key, [])).filter(keep));
  showIdle();
}

// ---------------------------------------------------------------------------
// Start screen

function showIdle() {
  ranked = hits = [];
  if (setup && !setup.model_installed) {
    const offerGpu = setup.gpu_supported && !setup.gpu_installed;
    const begin = (withGpu: boolean) => () => {
      gpuQueued = withGpu;
      openSettings();
      startDownload("model");
    };
    const get = el(
      "button",
      "accent",
      offerGpu
        ? t("setup.both", { size: megabytes(setup.model_size + setup.gpu_size) })
        : t("setup.model", { size: megabytes(setup.model_size) }),
    );
    get.prepend(icon("download"));
    get.addEventListener("click", begin(offerGpu));
    const extra: HTMLElement[] = [get];
    if (offerGpu) {
      const modelOnly = el("button", "secondary", t("setup.modelOnly", { size: megabytes(setup.model_size) }));
      modelOnly.addEventListener("click", begin(false));
      extra.push(modelOnly);
    }
    emptyState("download", t("setup.title"), offerGpu ? t("setup.detailGpu") : t("setup.detail"), ...extra);
    return;
  }
  if (!folderCount) {
    const add = el("button", "accent", t("drawer.add"));
    add.prepend(icon("plus"));
    add.addEventListener("click", addFolder);
    emptyState("folder", t("addFolder.title"), t("addFolder.detail"), add, el("span", "hint", t("addFolder.drag")));
    return;
  }

  const searches = keepHistory() ? recentSearches() : [];
  const files = keepHistory() ? recentFiles() : [];
  const chips = el("div", "suggestions");
  const runQuery = (text: string) => {
    queryEl.value = text;
    queryEl.focus();
    onQueryChanged();
  };
  for (const text of searches.length ? searches : SUGGESTIONS.map((k) => t(k))) {
    const chip = el("button", "chip-button");
    chip.append(icon(searches.length ? "clock" : "search"), el("span", undefined, text));
    chip.addEventListener("click", () => runQuery(text));
    if (searches.length) {
      const x = el("span", "forget");
      x.title = t("idle.forget");
      x.append(icon("close"));
      x.addEventListener("click", (e) => {
        e.stopPropagation();
        forget("recentSearches", (q: string) => q !== text);
      });
      chip.append(x);
    }
    chips.append(chip);
  }
  const extra: HTMLElement[] = [
    el("div", "idle-label", searches.length ? t("idle.recentSearches") : t("idle.try")),
    chips,
  ];

  if (files.length) {
    const list = el("div", "recent-files");
    for (const file of files) {
      const row = el("button", "recent-file");
      row.title = file.path;
      row.append(fileBadge(file.name, file.kind), el("span", "recent-name", file.name), el("span", "recent-where", locationOf(file.path)));
      row.addEventListener("click", () => openPath(file.path, file.name, false));
      const x = el("span", "forget");
      x.title = t("idle.forget");
      x.append(icon("close"));
      x.addEventListener("click", (e) => {
        e.stopPropagation();
        forget("recentFiles", (f: Recent) => f.path !== file.path);
      });
      row.append(x);
      list.append(row);
    }
    extra.push(el("div", "idle-label", t("idle.recentFiles")), list);
  }
  emptyState("search", t("idle.title"), t("idle.detail"), ...extra);
}

// ---------------------------------------------------------------------------
// Searching

async function runSearch() {
  const query = queryEl.value.trim();
  const seq = ++searchSeq;
  clearTimeout(rememberTimer);
  if (!query && !similarTo) {
    $("spinner").hidden = true;
    showIdle();
    return;
  }
  $("spinner").hidden = false;
  const started = performance.now();
  try {
    const found = similarTo
      ? await invoke<Hit[]>("similar", { path: similarTo.path, filter, filters: activeFilters(), limit })
      : await invoke<Hit[]>("search", { query, filter, filters: activeFilters(), limit });
    if (seq !== searchSeq) return;
    const keep = hits[selected]?.path;
    ranked = found;
    sortHits();
    // After "Show more" or a re-sort the same file stays selected.
    const again = hits.findIndex((h) => h.path === keep);
    selected = again >= 0 ? again : 0;
    renderHits();
    const ms = Math.round(performance.now() - started);
    metaEl.textContent = hits.length ? tn("meta.results", hits.length, { ms }) : "";
    if (query && hits.length) rememberTimer = window.setTimeout(() => rememberSearch(query), 2000);
  } catch (e) {
    if (seq === searchSeq) emptyState("close", t("searchFailed.title"), String(e));
  } finally {
    if (seq === searchSeq) $("spinner").hidden = true;
  }
}

/** Puts `ranked` in the order chosen in the toolbar. */
function sortHits() {
  const order = sortEl.value;
  hits = order === "relevance" ? [...ranked] : [...ranked].sort((a, b) => (order === "newest" ? b.modified - a.modified : a.modified - b.modified));
}

const STOP_WORDS = new Set(
  "a an and are as at be by do for from has have how in is it my of on or that the this to was what when where which who with".split(" "),
);

const CJK = /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}]/u;

/** The query's words as a pattern that finds them in text, or null when there are none. */
function queryPattern() {
  // Filler words would light up half of every snippet, so they are left plain.
  const words = queryEl.value
    .split(/[^\p{L}\p{N}]+/u)
    .filter((w) => (w.length > 1 || CJK.test(w)) && !STOP_WORDS.has(w.toLowerCase()));
  if (!words.length || similarTo) return null;
  // Latin words match whole words only, so "the" does not light up inside "clothes";
  // Chinese has no spaces between words, so it matches anywhere.
  const parts = words.map((w) => {
    const escaped = w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    return CJK.test(w) ? escaped : `(?<![\\p{L}\\p{N}])${escaped}(?![\\p{L}\\p{N}])`;
  });
  return new RegExp(`(${parts.join("|")})`, "giu");
}

/** Wraps the query's words in <mark> wherever they occur in `text`. */
function highlighted(text: string, className = "snippet", tag: "div" | "p" = "div") {
  const node = el(tag, className);
  const pattern = queryPattern();
  if (!pattern) {
    node.textContent = text;
    return node;
  }
  text.split(pattern).forEach((part, i) => node.append(i % 2 ? el("mark", undefined, part) : part));
  return node;
}

/**
 * How close a result is, in words and as a small meter. "Exact match" means every
 * query word occurs in the file; the other grades come from the similarity score.
 */
function matchLabel(hit: Hit) {
  const label = el("span", "match");
  // Cosine scores for this model sit between roughly 0.45 (unrelated) and 0.8 (very close).
  const strength = hit.score === null ? 1 : Math.min(1, Math.max(0.08, (hit.score - 0.45) / 0.35));
  let text: string;
  if (hit.keyword) {
    label.classList.add("strong");
    text = t("match.exact");
  } else {
    if (strength >= 0.7) label.classList.add("strong");
    text = strength >= 0.7 ? t("match.strong") : strength >= 0.4 ? t("match.good") : t("match.related");
  }
  const meter = el("span", "meter");
  const fill = el("span");
  fill.style.width = `${Math.round((hit.keyword ? Math.max(strength, 0.9) : strength) * 100)}%`;
  meter.append(fill);
  label.append(el("span", "match-text", text), meter);
  if (hit.score !== null) label.title = t("match.similarity", { score: hit.score.toFixed(2) });
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

/** What kind of file it is, in words: "PDF", "Word document", "PNG image". */
function kindLabel(name: string, kind: Hit["kind"]) {
  const ext = extension(name);
  if (kind === "image") return t("kind.image", { ext: ext.toUpperCase() });
  return t((FILE_TYPES[ext] ?? FILE_TYPES.txt)[2]);
}

/** The square with the file type's icon and colour. */
function fileBadge(name: string, kind: Hit["kind"]) {
  const ext = extension(name);
  const [type, iconName] = kind === "image" ? ["image", "image"] : (FILE_TYPES[ext] ?? FILE_TYPES.txt);
  const badge = el("div", "thumb filetype");
  badge.dataset.type = type;
  badge.append(icon(iconName), el("span", "ext", ext.slice(0, 4)));
  return badge;
}

/** Kind · page · date · where it lives, like the detail line in a file manager. */
function details(hit: Hit) {
  const ext = extension(hit.name);
  const page = hit.page ? t(ext === "pptx" ? "slide" : "page", { n: hit.page }) : "";
  const date = new Date(hit.modified * 1000).toLocaleDateString(lang);
  return [kindLabel(hit.name, hit.kind), page, date, locationOf(hit.path)].filter(Boolean).join("  ·  ");
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
    emptyState(similarTo ? "sparkle" : "search", t("noResults.title"), similarTo ? t("noSimilar.detail") : t("noResults.detail"));
    return;
  }
  const grid = filter === "images";
  resultsEl.className = "panel " + (grid ? "grid" : "list");
  document.body.classList.remove("no-results");

  // Rows are reused between searches while the same file stays in the results, so
  // typing does not rebuild the list and reload every picture on each keystroke.
  const next = new Map<string, HTMLElement>();
  const rows: HTMLElement[] = hits.map((hit) => {
    const key = [grid, hit.path, hit.page, hit.thumb, hit.snippet, hit.modified].join("\n");
    let row = rowCache.get(key);
    if (row) {
      row.classList.remove("selected");
      // Only the parts that depend on the query text need refreshing.
      row.querySelector(".snippet")?.replaceWith(highlighted(hit.snippet));
      row.querySelector(".match")?.replaceWith(matchLabel(hit));
    } else {
      row = buildRow(hit, grid);
    }
    row.dataset.index = String(next.size);
    next.set(key, row);
    return row;
  });
  rowCache = next;
  if (ranked.length >= limit && limit < MAX_RESULTS) {
    const more = el("button", "more", t("more"));
    more.addEventListener("click", () => {
      limit += PAGE;
      runSearch();
    });
    rows.push(more);
  }
  resultsEl.replaceChildren(...rows);
  select(Math.min(selected, hits.length - 1));
}

let rowCache = new Map<string, HTMLElement>();

function buildRow(hit: Hit, grid: boolean) {
  const row = el("div", grid ? "tile" : "hit");

  if (hit.kind === "image") {
    const img = el("img", "thumb");
    img.loading = "lazy";
    img.decoding = "async";
    // A small stored preview, not the original file.
    img.src = hit.thumb ? convertFileSrc(hit.thumb, "thumb") : convertFileSrc(hit.path);
    row.append(img);
  } else {
    row.append(fileBadge(hit.name, hit.kind));
  }

  const body = el("div", "body");
  body.append(el("div", "name", hit.name));
  if (hit.snippet && !grid) body.append(highlighted(hit.snippet));
  if (!grid) body.append(el("div", "path", details(hit)));
  row.append(body, matchLabel(hit));
  if (!grid) {
    const actions = el("div", "actions");
    actions.append(
      action("open", t("action.open"), () => openHit(hit, false)),
      action("folder", t("action.reveal"), () => openHit(hit, true)),
      action("sparkle", t("action.similar"), () => findSimilar(hit)),
    );
    row.append(actions);
  }
  row.title = hit.path;

  row.addEventListener("click", () => select(Number(row.dataset.index)));
  row.addEventListener("dblclick", (e) => openHit(hits[Number(row.dataset.index)], e.ctrlKey));
  return row;
}

function select(index: number) {
  selected = index;
  resultsEl.querySelector(".selected")?.classList.remove("selected");
  const row = resultsEl.querySelector<HTMLElement>(`[data-index="${index}"]`);
  row?.classList.add("selected");
  row?.scrollIntoView({ block: "nearest" });
  showPreview(hits[index] ?? null);
}

function openPath(path: string, name: string, reveal: boolean) {
  invoke(reveal ? "reveal_file" : "open_file", { path }).catch((e) =>
    toast(t("open.failed", { name, error: String(e) }), true),
  );
}

function openHit(hit: Hit | undefined, reveal: boolean) {
  if (!hit) return;
  openPath(hit.path, hit.name, reveal);
  rememberFile(hit);
  const query = queryEl.value.trim();
  if (query && !similarTo) rememberSearch(query);
}

/** Lists the files most like `hit`, in place of the search results. */
function findSimilar(hit: Hit) {
  similarTo = hit;
  // The list is about the file now, not the words typed.
  queryEl.value = "";
  $("clear").hidden = true;
  $("similar").hidden = false;
  $("similar-text").textContent = t("similar.to", { name: hit.name });
  $("similar").title = hit.path;
  limit = PAGE;
  selected = 0;
  runSearch();
}

function leaveSimilar() {
  if (!similarTo) return;
  similarTo = null;
  $("similar").hidden = true;
  limit = PAGE;
  runSearch();
}

$("similar-clear").addEventListener("click", () => {
  leaveSimilar();
  queryEl.focus();
});

// ---------------------------------------------------------------------------
// Preview pane

const previewCache = new Map<string, Promise<Preview>>();
let previewTimer: number | undefined;
let previewing: Hit | null = null;

function previewShown() {
  return !document.body.classList.contains("no-preview");
}

function showPreview(hit: Hit | null) {
  previewing = hit;
  clearTimeout(previewTimer);
  $("preview-empty").hidden = !!hit;
  $("preview-body").hidden = !hit;
  if (!hit || !previewShown()) return;

  $("preview-name").textContent = hit.name;
  $("preview-name").title = hit.path;
  $("preview-meta").textContent = details(hit);
  const media = $("preview-media");
  if (hit.kind === "image") {
    const img = el("img");
    img.decoding = "async";
    // The stored preview shows at once; the full picture replaces it when loaded.
    if (hit.thumb) img.src = convertFileSrc(hit.thumb, "thumb");
    const full = new Image();
    full.onload = () => {
      if (previewing === hit) img.src = full.src;
    };
    full.src = convertFileSrc(hit.path);
    media.className = "picture";
    media.replaceChildren(img);
  } else {
    media.className = "doc";
    media.replaceChildren(fileBadge(hit.name, hit.kind));
  }
  const textEl = $("preview-text");
  textEl.replaceChildren(el("p", "preview-note", t("preview.loading")));

  // Moving through the list quickly should not read every file on the way.
  previewTimer = window.setTimeout(async () => {
    const key = `${hit.path}\n${hit.modified}`;
    if (!previewCache.has(key)) {
      if (previewCache.size > 40) previewCache.clear();
      previewCache.set(key, invoke<Preview>("preview", { path: hit.path }));
    }
    try {
      const preview = await previewCache.get(key)!;
      if (previewing === hit) renderPreviewText(hit, preview);
    } catch (e) {
      previewCache.delete(key);
      if (previewing === hit) textEl.replaceChildren(el("p", "preview-note", t("preview.failed", { error: String(e) })));
    }
  }, 120);
}

function renderPreviewText(hit: Hit, preview: Preview) {
  const textEl = $("preview-text");
  const nodes: HTMLElement[] = [];
  const ext = extension(hit.name);
  let target: HTMLElement | null = null;
  for (const section of preview.sections) {
    const block = el("section");
    if (section.page !== null && preview.sections.length > 1) {
      block.append(el("div", "page-label", t(ext === "pptx" ? "slide" : "page", { n: section.page })));
    }
    for (const paragraph of section.text.split(/\n+/)) {
      if (paragraph.trim()) block.append(highlighted(paragraph, "para", "p"));
    }
    if (section.page !== null && section.page === hit.page) target = block;
    nodes.push(block);
  }
  if (!nodes.length && hit.kind !== "image") nodes.push(el("p", "preview-note", t("preview.none")));
  if (preview.truncated) nodes.push(el("p", "preview-note", t("preview.truncated")));
  textEl.replaceChildren(...nodes);
  textEl.scrollTop = 0;

  // Open where the result matched: the paragraph with the first highlighted word on
  // its page, or else the page itself.
  const goal = (target ?? textEl).querySelector("mark")?.closest(".para") ?? target;
  if (goal) textEl.scrollTop += goal.getBoundingClientRect().top - textEl.getBoundingClientRect().top - 8;
}

async function copyPreviewText() {
  const hit = previewing;
  if (!hit) return;
  let text = hit.snippet;
  try {
    const preview = await previewCache.get(`${hit.path}\n${hit.modified}`);
    const page = preview?.sections.find((s) => s.page === hit.page) ?? preview?.sections[0];
    if (page) text = page.text;
  } catch {
    // The snippet will do.
  }
  copy(text || hit.name, t("copied.text"));
}

$("pv-open").addEventListener("click", () => openHit(previewing ?? undefined, false));
$("pv-reveal").addEventListener("click", () => openHit(previewing ?? undefined, true));
$("pv-similar").addEventListener("click", () => previewing && findSimilar(previewing));
$("pv-copy-path").addEventListener("click", () => previewing && copy(previewing.path, t("copied.path")));
$("pv-copy-text").addEventListener("click", copyPreviewText);

function setPreviewShown(show: boolean) {
  document.body.classList.toggle("no-preview", !show);
  $("preview-toggle").classList.toggle("on", show);
  store("preview", show);
  if (show) showPreview(previewing);
}

$("preview-toggle").addEventListener("click", () => setPreviewShown(!previewShown()));
setPreviewShown(stored("preview", true));

// ---------------------------------------------------------------------------
// Folders

/** The line under a folder's path: how much of it is searchable. */
function folderState(folder: Folder) {
  const files = tn("folder.files", folder.files);
  if (folder.pending === null) return el("div", "count warn", indexing ? t("folder.waiting") : t("folder.notIndexed"));
  if (folder.pending > 0) {
    return el("div", "count warn", t(indexing ? "folder.toGo" : "folder.missing", { files, n: folder.pending }));
  }
  return el("div", "count", files);
}

/** Tells the user when search cannot see everything yet, with the way out. */
function updateBanner() {
  const banner = $("banner");
  const button = $("banner-action");
  const incomplete = folders.filter((f) => f.pending !== 0);
  banner.hidden = !indexing && !incomplete.length;
  banner.classList.toggle("warn", !indexing);
  banner.querySelector(".icon")!.innerHTML = ICONS[indexing ? "info" : "warn"];
  if (indexing) {
    $("banner-text").textContent = t("banner.indexing");
    button.textContent = t("banner.stop");
  } else if (incomplete.length) {
    const names = incomplete.map((f) => folderName(f.path)).join(", ");
    $("banner-text").textContent = t("banner.incomplete", { names });
    button.textContent = t("banner.resume");
  }
}

async function refreshFolders() {
  const hadFolders = folderCount > 0;
  folders = await invoke<Folder[]>("list_folders");
  folderCount = folders.length;
  $("folder-count").hidden = !folderCount;
  $("folder-count").textContent = String(folderCount);
  if (!folders.length) {
    foldersEl.replaceChildren(el("li", "none", t("drawer.none")));
  } else {
    foldersEl.replaceChildren(
      ...folders.map((folder) => {
        const li = el("li");
        const info = el("div", "folder-info");
        info.append(el("div", "folder-name", folderName(folder.path)), el("div", "path", folder.path), folderState(folder));
        const remove = el("button", "subtle danger");
        remove.title = t("drawer.remove");
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
  fillFolderFilter();
  if (hadFolders !== folderCount > 0 && !queryEl.value.trim() && !similarTo) showIdle();
}

/** Indexing phases come from the backend in English. */
function phaseText(phase: string) {
  const key = `phase.${phase}`;
  return has(key) ? t(key) : phase;
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
    const phase = phaseText(p.phase);
    statusEl.textContent =
      (p.total ? t("status.progress", { phase, done: p.done, total: p.total }) : phase) + (file ? ` · ${file}` : "");
  } else if (p.error) {
    statusEl.textContent = t("status.error", { error: p.error });
  } else {
    statusEl.textContent = tn("status.indexed", s.files);
  }

  const backend = $("backend");
  backend.hidden = !s.backend;
  const gpuAvailable = !!setup && setup.gpu_supported && !setup.gpu_installed;
  backend.textContent = s.backend === "CUDA" ? t("backend.gpu") : gpuAvailable ? t("backend.cpuGpu") : s.backend;
  backend.classList.toggle("link", s.backend !== "CUDA" && gpuAvailable);
  backend.classList.toggle("on", s.engine_loaded);
  backend.title = s.engine_loaded ? t("backend.loaded", { backend: s.backend }) : t("backend.unloaded");

  // Folder counts while a pass runs, and fresh results once it finishes.
  if (p.running || wasIndexing) await refreshFolders();
  else updateBanner();
  if (wasIndexing && !p.running) runSearch();
  wasIndexing = p.running;
}

async function addFolder() {
  const path = await open({ directory: true, title: t("drawer.choose") });
  if (typeof path === "string") addFolders([path]);
}

/** Adds each path that is a folder and starts indexing; anything else is reported. */
async function addFolders(paths: string[]) {
  const added: string[] = [];
  let problem = "";
  for (const path of paths) {
    try {
      await invoke("add_folder", { path });
      added.push(folderName(path));
    } catch (e) {
      // Not a folder, or already covered by a folder in the list.
      const text = String(e);
      const covered = text.match(/^Already searched as part of (.*)$/);
      problem = text.startsWith("not a folder")
        ? t("add.notFolder")
        : covered
          ? t("add.covered", { folder: covered[1] })
          : text;
    }
  }
  if (added.length) toast(t("add.started", { names: added.join(", ") }));
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
  // Typing starts a new search, out of the similar files.
  if (similarTo && queryEl.value.trim()) {
    similarTo = null;
    $("similar").hidden = true;
  }
  limit = PAGE;
  clearTimeout(debounce);
  debounce = window.setTimeout(runSearch, 180);
}

queryEl.addEventListener("input", onQueryChanged);

$("clear").addEventListener("click", () => {
  queryEl.value = "";
  queryEl.focus();
  onQueryChanged();
});

sortEl.value = stored("sort", "relevance");
sortEl.addEventListener("change", () => {
  store("sort", sortEl.value);
  const keep = hits[selected]?.path;
  sortHits();
  selected = Math.max(0, hits.findIndex((h) => h.path === keep));
  if (hits.length) renderHits();
});

window.addEventListener("keydown", (e) => {
  // The dialog handles its own keys, including Esc to close.
  if (about.open || settings.open) return;
  if (e.key === "Escape") {
    if (drawer.classList.contains("open")) setDrawer(false);
    else if (similarTo) leaveSimilar();
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
  } else if (e.key.toLowerCase() === "c" && e.ctrlKey && e.shiftKey) {
    copy(hits[selected].path, t("copied.path"));
    e.preventDefault();
  }
});

$("filters").addEventListener("click", (e) => {
  const button = (e.target as HTMLElement).closest("button");
  if (!button) return;
  filter = button.dataset.filter ?? "all";
  for (const b of $("filters").querySelectorAll("button")) b.classList.toggle("active", b === button);
  queryEl.focus();
  limit = PAGE;
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
  $("about-version").textContent = t("about.version", { version });
  const device = s.backend === "CUDA" ? t("about.gpu") : s.backend;
  $("about-engine").textContent = !s.backend
    ? t("about.notStarted")
    : s.engine_loaded
      ? device
      : t("about.idle", { device });
  $("about-index").textContent = t("about.indexSize", {
    files: tn("folder.files", s.files),
    folders: tn("about.folders", folderCount),
  });
  if (!about.open) about.showModal();
}

$("about-open").addEventListener("click", showAbout);
$("about-close").addEventListener("click", () => about.close());
// A click on the dimmed area around a dialog closes it.
for (const dialog of [about, settings]) {
  dialog.addEventListener("click", (e) => {
    if (e.target === dialog) dialog.close();
  });
}
listen("show-about", showAbout);

// ---------------------------------------------------------------------------
// Model and GPU downloads

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
/** GPU support was chosen together with the model: fetch it once the model is in. */
let gpuQueued = false;
let wasDownloading = false;

/** Download steps come from the backend in English: "Downloading model.gguf". */
function stepText(step: string) {
  const [word, ...rest] = step.split(" ");
  const file = rest.join(" ");
  const key = `step.${word}`;
  return has(key) ? t(key, { file }) : step;
}

/** Reads what is installed and any download in flight, and reflects it in Settings. */
async function refreshSetup() {
  const hadModel = setup?.model_installed;
  setup = await invoke<Setup>("setup_status");
  const d = setup.download;

  $("gpu-state").textContent = setup.gpu_installed
    ? t("set.gpuInstalled")
    : setup.gpu_supported
      ? t("set.gpuFound")
      : t("set.gpuNone");
  const gpuAction = $<HTMLButtonElement>("gpu-action");
  gpuAction.hidden = setup.gpu_installed || !setup.gpu_supported;
  gpuAction.textContent = t("set.gpuDownload", { size: megabytes(setup.gpu_size) });
  gpuAction.disabled = d.running;

  $("model-state").textContent = setup.model_installed ? t("set.modelInstalled") : t("set.modelMissing");
  const modelAction = $<HTMLButtonElement>("model-action");
  modelAction.hidden = setup.model_installed;
  modelAction.textContent = t("set.modelDownload", { size: megabytes(setup.model_size) });
  modelAction.disabled = d.running;

  $("download").hidden = !d.running;
  if (d.running) {
    $("download-fill").style.width = `${d.total ? (d.done / d.total) * 100 : 0}%`;
    $("download-text").textContent = t("download.progress", {
      step: stepText(d.step),
      done: megabytes(d.done),
      total: megabytes(d.total),
    });
  }
  const failed = $("download-error");
  failed.hidden = d.running || !d.error;
  failed.textContent = d.error ? t("download.failed", { error: d.error }) : "";

  if (wasDownloading && !d.running && !d.error && !d.cancelled) {
    toast(d.component === "gpu" ? t("download.gpuDone") : t("download.modelDone"));
    if (d.component === "model" && gpuQueued) {
      gpuQueued = false;
      startDownload("gpu");
    }
    // Folders added before the model arrived can be indexed now.
    if (d.component === "model") invoke("reindex");
    refreshStatus();
  }
  wasDownloading = d.running;
  if (d.cancelled || d.error) gpuQueued = false;

  // Tell people with an NVIDIA card that GPU support exists, until they take it or decline.
  const tipWanted =
    setup.model_installed && setup.gpu_supported && !setup.gpu_installed && !d.running && !gpuQueued && !stored("gpuTipDismissed", false);
  $("tip").hidden = !tipWanted;
  $("tip-action").textContent = t("tip.download", { size: megabytes(setup.gpu_size) });
  if (hadModel !== setup.model_installed && !queryEl.value.trim() && !similarTo) showIdle();
}

async function startDownload(component: "gpu" | "model") {
  try {
    await invoke("download_component", { component });
  } catch (e) {
    toast(String(e), true);
  }
  refreshSetup();
}

$("tip-action").addEventListener("click", () => {
  openSettings();
  startDownload("gpu");
});
$("tip-dismiss").addEventListener("click", () => {
  store("gpuTipDismissed", true);
  $("tip").hidden = true;
});
// The status bar's CPU label leads to the GPU option when one is available.
$("backend").addEventListener("click", () => {
  if ($("backend").classList.contains("link")) openSettings();
});

function openSettings() {
  refreshSetup();
  loadIndexSettings();
  $<HTMLInputElement>("history-keep").checked = keepHistory();
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

// ---------------------------------------------------------------------------
// Updates

/** A newer release found on GitHub, if any. */
let update: Update | null = null;
let updateChecked = false;
let updateDismissed = false;
let installing = false;
let portable = false;
const UPDATE_INTERVAL = 24 * 60 * 60 * 1000;

/** Automatic checks are on unless turned off in Settings. */
function autoUpdate() {
  try {
    return localStorage.getItem("autoUpdate") !== "0";
  } catch {
    return true;
  }
}

/** Asks GitHub for the latest release; `quiet` keeps failures out of sight. */
async function checkForUpdate(quiet: boolean) {
  try {
    update = await check();
    updateChecked = true;
  } catch (e) {
    if (!quiet) toast(t("update.checkFailed", { error: String(e) }), true);
    return;
  }
  showUpdate();
}

/** Reflects the latest check in the notice bar and in Settings. */
async function showUpdate() {
  const current = await getVersion();
  const label = portable ? t("update.download") : t("update.install");
  $("update").hidden = !update || updateDismissed || installing;
  $("update-text").textContent = t("update.available", { version: update?.version ?? "" });
  $("update-action").textContent = label;
  $("update-state").textContent = update
    ? t("set.updateAvailable", { version: update.version, current })
    : updateChecked
      ? t("set.updateLatest", { current })
      : t("set.updateVersion", { current });
  const install = $<HTMLButtonElement>("update-install");
  install.hidden = !update;
  install.textContent = label;
  $<HTMLInputElement>("update-auto").checked = autoUpdate();
}

/** Installs the update, which closes NLsearch and reopens it; a portable copy gets the download page. */
async function installUpdate() {
  if (!update || installing) return;
  if (portable) {
    invoke("open_downloads");
    return;
  }
  installing = true;
  const buttons = [$<HTMLButtonElement>("update-action"), $<HTMLButtonElement>("update-install")];
  let total = 0;
  let done = 0;
  try {
    for (const b of buttons) b.disabled = true;
    await update.downloadAndInstall((event) => {
      if (event.event === "Started") total = event.data.contentLength ?? 0;
      else if (event.event === "Progress") done += event.data.chunkLength;
      const text =
        event.event === "Finished"
          ? t("update.installing")
          : total
            ? t("update.percent", { n: Math.round((done / total) * 100) })
            : t("update.downloading");
      for (const b of buttons) b.textContent = text;
    });
  } catch (e) {
    toast(t("update.failed", { error: String(e) }), true);
  } finally {
    installing = false;
    for (const b of buttons) b.disabled = false;
    showUpdate();
  }
}

$("update-action").addEventListener("click", installUpdate);
$("update-install").addEventListener("click", installUpdate);
$("update-dismiss").addEventListener("click", () => {
  updateDismissed = true;
  $("update").hidden = true;
});
$("update-check").addEventListener("click", () => checkForUpdate(false));
$("update-auto").addEventListener("change", (e) => {
  const on = (e.target as HTMLInputElement).checked;
  try {
    localStorage.setItem("autoUpdate", on ? "1" : "0");
  } catch {
    // Without storage the default applies next time.
  }
  if (on) checkForUpdate(true);
});

invoke<boolean>("is_portable").then((p) => {
  portable = p;
  showUpdate();
  if (autoUpdate()) checkForUpdate(true);
});
// NLsearch lives in the tray for days, so look again once a day.
setInterval(() => {
  if (autoUpdate()) checkForUpdate(true);
}, UPDATE_INTERVAL);

// ---------------------------------------------------------------------------
// Filters: date, folder and file type, on top of the kind tabs.

const dateFilter = $<HTMLSelectElement>("filter-date");
const folderFilter = $<HTMLSelectElement>("filter-folder");
const typeFilter = $<HTMLSelectElement>("filter-type");

typeFilter.append(new Option(t("filter.allTypes"), ""));
for (const [i, [label]] of TYPE_GROUPS.entries()) typeFilter.append(new Option(t(label), String(i)));

/** What the filter bar asks for, in the form the search command takes. */
function activeFilters() {
  const days = Number(dateFilter.value);
  return {
    since: days ? Math.floor(Date.now() / 1000) - days * 86400 : null,
    folder: folderFilter.value ? Number(folderFilter.value) : null,
    extensions: typeFilter.value ? TYPE_GROUPS[Number(typeFilter.value)][1] : [],
  };
}

/** The folder choices follow the folder list; a removed folder's choice is dropped. */
function fillFolderFilter() {
  const chosen = folderFilter.value;
  folderFilter.replaceChildren(new Option(t("filter.allFolders"), ""));
  for (const folder of folders) folderFilter.append(new Option(folderName(folder.path), String(folder.id)));
  folderFilter.value = folders.some((f) => String(f.id) === chosen) ? chosen : "";
  showFilterState();
}

/** The filter button lights up while a filter is narrowing the results. */
function showFilterState() {
  const active = !!(dateFilter.value || folderFilter.value || typeFilter.value);
  $("filters-toggle").classList.toggle("on", active || !$("filterbar").hidden);
  $("filters-clear").hidden = !active;
}

for (const select of [dateFilter, folderFilter, typeFilter]) {
  select.addEventListener("change", () => {
    showFilterState();
    limit = PAGE;
    runSearch();
  });
}
$("filters-toggle").addEventListener("click", () => {
  $("filterbar").hidden = !$("filterbar").hidden;
  showFilterState();
});
$("filters-clear").addEventListener("click", () => {
  dateFilter.value = folderFilter.value = typeFilter.value = "";
  showFilterState();
  runSearch();
});

// ---------------------------------------------------------------------------
// Settings that change what is indexed, and start-up.

interface IndexSettings {
  exclude: string[];
  ocr: boolean;
  ocr_languages: string[];
  ocr_available: boolean;
}

let indexSettings: IndexSettings | null = null;

async function loadIndexSettings() {
  indexSettings = await invoke<IndexSettings>("get_settings");
  $<HTMLTextAreaElement>("exclude").value = indexSettings.exclude.join("\n");
  const ocr = $<HTMLInputElement>("ocr");
  ocr.checked = indexSettings.ocr;
  ocr.disabled = !indexSettings.ocr_available && !indexSettings.ocr;
  $("ocr-state").textContent = indexSettings.ocr_available
    ? t("set.ocrAvailable", { languages: indexSettings.ocr_languages.join(", ") })
    : t("set.ocrMissing");
  try {
    $<HTMLInputElement>("autostart").checked = await autostartEnabled();
  } catch {
    $<HTMLInputElement>("autostart").disabled = true;
  }
}

async function saveIndexSettings(changes: Partial<IndexSettings>, message: string) {
  if (!indexSettings) return;
  const next = { exclude: indexSettings.exclude, ocr: indexSettings.ocr, ...changes };
  try {
    await invoke("save_settings", { settings: { exclude: next.exclude, ocr: next.ocr } });
    Object.assign(indexSettings, next);
    toast(message);
    refreshStatus();
  } catch (e) {
    toast(t("save.failed", { error: String(e) }), true);
  }
}

$("exclude-save").addEventListener("click", () => {
  const rules = $<HTMLTextAreaElement>("exclude")
    .value.split("\n")
    .map((r) => r.trim())
    .filter(Boolean);
  saveIndexSettings({ exclude: rules }, t("set.excludeSaved"));
});
$("ocr").addEventListener("change", (e) => {
  const on = (e.target as HTMLInputElement).checked;
  saveIndexSettings({ ocr: on }, on ? t("set.ocrStarted") : t("set.ocrStopped"));
});
$("autostart").addEventListener("change", async (e) => {
  const on = (e.target as HTMLInputElement).checked;
  try {
    await (on ? enableAutostart() : disableAutostart());
  } catch (err) {
    toast(t("set.autostartFailed", { error: String(err) }), true);
    (e.target as HTMLInputElement).checked = !on;
  }
});

$("history-keep").addEventListener("change", (e) => {
  store("keepHistory", (e.target as HTMLInputElement).checked);
  if (!queryEl.value.trim() && !similarTo) showIdle();
});
$("history-clear").addEventListener("click", () => {
  store("recentSearches", []);
  store("recentFiles", []);
  toast(t("set.historyCleared"));
  if (!queryEl.value.trim() && !similarTo) showIdle();
});

// ---------------------------------------------------------------------------
// Language: the page is translated as it loads, so a change reloads it.

const languageEl = $<HTMLSelectElement>("language");
languageEl.value = languageChoice();
languageEl.addEventListener("change", () => {
  setLanguageChoice(languageEl.value as LanguageChoice);
  location.reload();
});
invoke("set_tray_labels", { show: t("tray.show"), about: t("tray.about"), quit: t("tray.quit") }).catch(() => {
  // The tray keeps its English labels.
});

// ---------------------------------------------------------------------------
// Appearance: glass or paper, each light or dark, and light or dark following Windows.

type ThemeChoice = "system" | "light" | "dark";
type Style = "glass" | "paper";
const systemDark = window.matchMedia("(prefers-color-scheme: dark)");

function themeChoice(): ThemeChoice {
  const saved = stored<string>("themeChoice", "system");
  return saved === "light" || saved === "dark" ? saved : "system";
}

function styleChoice(): Style {
  return stored<string>("style", "glass") === "paper" ? "paper" : "glass";
}

function applyTheme(choice: ThemeChoice) {
  const dark = choice === "dark" || (choice === "system" && systemDark.matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
  document.documentElement.dataset.style = styleChoice();
  for (const b of document.querySelectorAll<HTMLElement>("#theme button")) {
    const on = b.dataset.theme === choice;
    b.classList.toggle("active", on);
    b.setAttribute("aria-checked", String(on));
  }
  for (const b of document.querySelectorAll<HTMLElement>("#style button")) {
    const on = b.dataset.style === styleChoice();
    b.classList.toggle("active", on);
    b.setAttribute("aria-checked", String(on));
  }
}

/** The title bar follows the chosen look too; null hands it back to Windows. */
async function chooseTheme(choice: ThemeChoice) {
  store("themeChoice", choice);
  try {
    await getCurrentWindow().setTheme(choice === "system" ? null : choice);
  } catch {
    // An older shell without the permission keeps its title bar as it is.
  }
  // Once the window theme is back on the system's, the media query reports Windows' own.
  applyTheme(choice);
}

for (const b of document.querySelectorAll<HTMLElement>("#theme button")) {
  b.addEventListener("click", () => chooseTheme(b.dataset.theme as ThemeChoice));
}
for (const b of document.querySelectorAll<HTMLElement>("#style button")) {
  b.addEventListener("click", () => {
    store("style", b.dataset.style);
    applyTheme(themeChoice());
  });
}
systemDark.addEventListener("change", () => applyTheme(themeChoice()));
applyTheme(themeChoice());
chooseTheme(themeChoice());

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

refreshSetup().then(refreshFolders).then(() => {
  if (!queryEl.value.trim() && !similarTo) showIdle();
});
refreshStatus();
setInterval(refreshStatus, 1500);
