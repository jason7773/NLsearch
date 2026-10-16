//! Finds the files worth indexing and turns each into the passages or image to embed.

use std::io::{Cursor, Read};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use base64::Engine as _;
use image::ImageDecoder;

use crate::embed::Input;
use crate::text::{self, chunk_text};

/// Longest image side sent to the model.
const IMAGE_MAX_SIDE: u32 = 896;
/// Longest side of the stored preview shown in results.
const THUMB_MAX_SIDE: u32 = 360;
/// Longest image side handed to OCR: enough for screenshot text, without slowing it down.
const OCR_MAX_SIDE: u32 = 2400;
/// Recognised text shorter than this (letters and digits) is noise from a photo, not text.
const MIN_OCR_CHARS: usize = 20;
/// Scanned pages read per PDF; OCR takes about a second a page.
const MAX_OCR_PAGES: usize = 50;

const MAX_TEXT_BYTES: u64 = 5 * 1024 * 1024;
const MAX_OFFICE_BYTES: u64 = 50 * 1024 * 1024;
const MAX_PDF_BYTES: u64 = 100 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 50 * 1024 * 1024;

const SKIP_DIRS: &[&str] = &[
    "node_modules", "target", "__pycache__", "$RECYCLE.BIN", "System Volume Information",
    "AppData", "venv", "dist", "build",
];

/// Source code and configuration, read as plain text.
const CODE_EXTS: &[&str] = &[
    "py", "js", "mjs", "cjs", "ts", "tsx", "jsx", "java", "kt", "kts", "scala", "c", "h", "cpp", "cc",
    "hpp", "cs", "go", "rs", "rb", "php", "swift", "dart", "lua", "pl", "r", "sql", "sh", "ps1", "bat",
    "cmd", "json", "yaml", "yml", "toml", "xml", "ini", "cfg", "conf", "css", "scss", "vue", "svelte",
    "ipynb",
];

pub fn kind_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "txt" | "md" | "markdown" | "rst" | "log" | "csv" | "html" | "htm" | "docx" | "xlsx" | "pptx" => "text",
        "pdf" => "pdf",
        "jpg" | "jpeg" | "png" | "webp" | "bmp" | "gif" => "image",
        ext if CODE_EXTS.contains(&ext) => "text",
        _ => return None,
    })
}

/// Folders and files the user chose to leave out, on top of the built-in `SKIP_DIRS`.
#[derive(Clone, Default)]
pub struct Exclusions {
    names: Vec<String>,
    paths: Vec<String>,
}

impl Exclusions {
    /// One rule per entry. A full path such as `D:\Photos\Old` leaves out that folder or
    /// file; anything else is a name, with `*` and `?` wildcards, matched against every
    /// folder and file name, such as `Archive` or `*.tmp`. Case does not matter.
    pub fn new(rules: &[String]) -> Exclusions {
        let mut exclusions = Exclusions::default();
        for rule in rules.iter().map(|r| r.trim()).filter(|r| !r.is_empty()) {
            if rule.contains(['/', '\\', ':']) {
                exclusions.paths.push(comparable(rule));
            } else {
                exclusions.names.push(rule.to_lowercase());
            }
        }
        exclusions
    }

    /// Whether a folder or file with this name, at this path, is left out.
    fn skips(&self, name: &str, path: &Path) -> bool {
        if name.starts_with('.') || SKIP_DIRS.contains(&name) {
            return true;
        }
        let name = name.to_lowercase();
        if self.names.iter().any(|pattern| wildcard_match(pattern, &name)) {
            return true;
        }
        if self.paths.is_empty() {
            return false;
        }
        let path = comparable(&path.to_string_lossy());
        self.paths.iter().any(|p| path == *p || path.starts_with(&format!("{p}/")))
    }

    /// Whether anything on the way from `root` down to `path` is left out. The root
    /// itself is always indexed: the user picked it.
    pub fn excludes(&self, root: &Path, path: &Path) -> bool {
        let Ok(inner) = path.strip_prefix(root) else { return true };
        let mut current = root.to_path_buf();
        inner.components().any(|part| {
            current.push(part);
            self.skips(&part.as_os_str().to_string_lossy(), &current)
        })
    }
}

/// A path for comparing: forward slashes, no trailing slash, lower case.
fn comparable(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// `*` matches any run of characters and `?` any one character.
fn wildcard_match(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni) = (0, 0);
    // Where the last `*` was, and how much of the name it had taken.
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ni));
            pi += 1;
        } else if let Some((sp, sn)) = star {
            pi = sp + 1;
            ni = sn + 1;
            star = Some((sp, sn + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// Reads text in pictures and in PDF pages that have no text layer. The app provides
/// Windows' built-in OCR when the user turns it on.
pub trait Ocr: Sync {
    fn image(&self, img: &image::RgbaImage) -> Result<String>;
    /// `page` counts from 0.
    fn pdf_page(&self, path: &Path, page: usize) -> Result<String>;
}

/// Whether OCR output is real text rather than a few stray marks found in a photo.
fn is_text(recognised: &str) -> bool {
    recognised.chars().filter(|c| c.is_alphanumeric()).count() >= MIN_OCR_CHARS
}

pub struct Candidate {
    pub path: PathBuf,
    pub mtime: i64,
    pub size: u64,
    pub kind: &'static str,
    /// Only in the cloud (OneDrive and other sync apps): reading it downloads it.
    pub cloud: bool,
}

pub struct Item {
    pub page: Option<i64>,
    /// Stored for snippets and keyword search.
    pub text: String,
    pub input: Input,
}

pub fn scan(root: &Path, exclusions: &Exclusions) -> Vec<Candidate> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !exclusions.skips(&e.file_name().to_string_lossy(), e.path()))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let kind = kind_for(e.path())?;
            let meta = e.metadata().ok()?;
            let ext = e.path().extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
            let limit = match kind {
                "pdf" => MAX_PDF_BYTES,
                "image" => MAX_IMAGE_BYTES,
                _ if ["docx", "xlsx", "pptx"].contains(&ext.as_str()) => MAX_OFFICE_BYTES,
                _ => MAX_TEXT_BYTES,
            };
            if meta.len() == 0 || meta.len() > limit {
                return None;
            }
            let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
            let cloud = is_cloud_only(&meta);
            Some(Candidate { path: e.into_path(), mtime, size: meta.len(), kind, cloud })
        })
        .collect()
}

/// Windows marks a file whose content is not on this PC yet. Placeholders made by
/// OneDrive and other sync apps carry RECALL_ON_DATA_ACCESS; older ones OFFLINE.
#[cfg(windows)]
pub fn is_cloud_only(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
    const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x40000;
    const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x400000;
    meta.file_attributes() & (FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS) != 0
}

#[cfg(not(windows))]
pub fn is_cloud_only(_meta: &std::fs::Metadata) -> bool {
    false
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

/// The passages or image to embed for a file, plus a preview for images. With `ocr`,
/// text in pictures and scanned PDF pages is read as well.
pub fn extract(file: &Candidate, ocr: Option<&dyn Ocr>) -> Result<(Vec<Item>, Option<Vec<u8>>)> {
    let name = file.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let passage = |page: Option<i64>, text: String| Item {
        page,
        input: Input::Text(format!("title: {name} | text: {text}")),
        text,
    };

    match file.kind {
        "image" => {
            let img = open_image(&file.path)?;
            let model_size = img.thumbnail(IMAGE_MAX_SIDE, IMAGE_MAX_SIDE);
            let b64 = base64::engine::general_purpose::STANDARD.encode(encode_jpeg(&model_size, 85)?);
            let thumb = encode_jpeg(&model_size.thumbnail(THUMB_MAX_SIDE, THUMB_MAX_SIDE), 80)?;
            // The file name is what keyword search matches for an image.
            let mut items = vec![Item { page: None, text: name.clone(), input: Input::Image(b64) }];
            if let Some(ocr) = ocr {
                let readable = if img.width().max(img.height()) > OCR_MAX_SIDE {
                    img.thumbnail(OCR_MAX_SIDE, OCR_MAX_SIDE)
                } else {
                    img
                };
                // A picture OCR cannot read is still indexed as a picture.
                if let Ok(found) = ocr.image(&readable.to_rgba8()) {
                    if is_text(&found) {
                        items.extend(chunk_text(&found).into_iter().map(|c| passage(None, c)));
                    }
                }
            }
            Ok((items, Some(thumb)))
        }
        "pdf" => {
            // pdf-extract panics on some malformed files.
            let mut pages = catch_unwind(AssertUnwindSafe(|| pdf_extract::extract_text_by_pages(&file.path)))
                .map_err(|_| anyhow::anyhow!("pdf parser panicked"))??;
            if let Some(ocr) = ocr {
                // Pages without a text layer are scans: read the picture instead.
                let scanned = pages.iter().enumerate().filter(|(_, text)| !is_text(text)).map(|(i, _)| i);
                let scanned: Vec<usize> = scanned.take(MAX_OCR_PAGES).collect();
                for i in scanned {
                    if let Ok(found) = ocr.pdf_page(&file.path, i) {
                        if is_text(&found) {
                            pages[i] = found;
                        }
                    }
                }
            }
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
            // A slide deck is indexed slide by slide, so results can name the slide.
            if ext == "pptx" {
                let items = pptx_slides(&file.path)?
                    .into_iter()
                    .enumerate()
                    .flat_map(|(i, text)| chunk_text(&text).into_iter().map(move |c| (Some(i as i64 + 1), c)))
                    .map(|(page, c)| passage(page, c))
                    .collect();
                return Ok((items, None));
            }
            let text = match ext.as_str() {
                "docx" => docx_text(&file.path)?,
                "xlsx" => xlsx_text(&file.path)?,
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

fn zip_part(archive: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Result<String> {
    let mut xml = String::new();
    archive.by_name(name)?.read_to_string(&mut xml)?;
    Ok(xml)
}

/// Parts named `<prefix><number>.xml`, such as `ppt/slides/slide3.xml`, in number order.
fn numbered_parts(archive: &zip::ZipArchive<std::fs::File>, prefix: &str) -> Vec<String> {
    let mut parts: Vec<(u32, String)> = archive
        .file_names()
        .filter_map(|n| Some((n.strip_prefix(prefix)?.strip_suffix(".xml")?.parse().ok()?, n.to_string())))
        .collect();
    parts.sort();
    parts.into_iter().map(|(_, n)| n).collect()
}

fn docx_text(path: &Path) -> Result<String> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let xml = zip_part(&mut archive, "word/document.xml").context("not a docx")?;
    Ok(ooxml_text(&xml, "w:t", "w:p"))
}

/// The text of each slide, in order.
fn pptx_slides(path: &Path) -> Result<Vec<String>> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let slides = numbered_parts(&archive, "ppt/slides/slide");
    anyhow::ensure!(!slides.is_empty(), "not a pptx");
    slides.iter().map(|part| Ok(ooxml_text(&zip_part(&mut archive, part)?, "a:t", "a:p"))).collect()
}

/// Every sheet's cells, one line per row, cells separated by tabs.
fn xlsx_text(path: &Path) -> Result<String> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    // Cell text is usually stored once in a shared table and referred to by number.
    let shared: Vec<String> = match zip_part(&mut archive, "xl/sharedStrings.xml") {
        Ok(xml) => xml.split("</si>").map(|si| ooxml_text(si, "t", "si")).collect(),
        Err(_) => Vec::new(),
    };
    let sheets = numbered_parts(&archive, "xl/worksheets/sheet");
    anyhow::ensure!(!sheets.is_empty(), "not an xlsx");
    let mut out = String::new();
    for part in &sheets {
        let xml = zip_part(&mut archive, part)?;
        for row in xml.split("</row>") {
            let cells: Vec<String> = row.split("<c ").skip(1).filter_map(|cell| xlsx_cell(cell, &shared)).collect();
            if !cells.is_empty() {
                out.push_str(&cells.join("\t"));
                out.push('\n');
            }
        }
    }
    Ok(out)
}

/// The value of one cell, given the XML after its `<c `.
fn xlsx_cell(cell: &str, shared: &[String]) -> Option<String> {
    let open_end = cell.find('>')?;
    let attrs = &cell[..open_end];
    if attrs.ends_with('/') {
        return None;
    }
    let body = &cell[open_end + 1..cell.find("</c>").unwrap_or(cell.len())];
    let kind = attrs.split(" t=\"").nth(1).and_then(|rest| rest.split('"').next()).unwrap_or("n");
    let value = match kind {
        "inlineStr" => ooxml_text(body, "t", "is"),
        "b" => return None,
        _ => {
            let raw = body.split("<v>").nth(1)?.split("</v>").next()?;
            if kind == "s" {
                shared.get(raw.parse::<usize>().ok()?)?.clone()
            } else {
                unescape(raw)
            }
        }
    };
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// The text of an Office Open XML part: the contents of every `<tag>` element (the
/// text runs), with a line break after every `</para>`.
fn ooxml_text(xml: &str, tag: &str, para: &str) -> String {
    let (open, close, para_end) = (format!("<{tag}"), format!("</{tag}>"), format!("</{para}>"));
    let mut out = String::new();
    let mut pos = 0;
    loop {
        let run = xml[pos..].find(&open).map(|i| pos + i);
        let end = xml[pos..].find(&para_end).map(|i| pos + i);
        match (run, end) {
            (Some(r), e) if e.map_or(true, |e| r < e) => {
                let after = r + open.len();
                let Some(gt) = xml[after..].find('>') else { break };
                let attrs = &xml[after..after + gt];
                // `<a:t>` or `<a:t xml:space="preserve">`, but not `<a:tab/>` or `<a:tbl>`.
                if !(attrs.is_empty() || attrs.starts_with(' ')) || attrs.ends_with('/') {
                    pos = after;
                    continue;
                }
                let start = after + gt + 1;
                let Some(len) = xml[start..].find(&close) else { break };
                out.push_str(&unescape(&xml[start..start + len]));
                pos = start + len + close.len();
            }
            (_, Some(e)) => {
                out.push('\n');
                pos = e + para_end.len();
            }
            _ => break,
        }
    }
    out
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
    unescape(&out)
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn texts(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.text.trim()).collect()
    }

    #[test]
    fn kinds_follow_the_extension() {
        assert_eq!(kind_for(Path::new("a/Notes.MD")), Some("text"));
        assert_eq!(kind_for(Path::new("report.pdf")), Some("pdf"));
        assert_eq!(kind_for(Path::new("photo.JPEG")), Some("image"));
        assert_eq!(kind_for(Path::new("app.exe")), None);
        assert_eq!(kind_for(Path::new("README")), None);
        assert_eq!(kind_for(Path::new("main.rs")), Some("text"));
        assert_eq!(kind_for(Path::new("Budget.xlsx")), Some("text"));
    }

    #[test]
    fn scan_skips_hidden_build_and_empty_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for (path, body) in [
            ("keep.txt", "hello"),
            ("sub/keep.md", "# title"),
            ("empty.txt", ""),
            ("program.exe", "MZ"),
            (".git/config.txt", "x"),
            ("node_modules/pkg/readme.md", "x"),
        ] {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }
        let mut found: Vec<String> = scan(root, &Exclusions::default())
            .iter()
            .map(|c| c.path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"))
            .collect();
        found.sort();
        assert_eq!(found, ["keep.txt", "sub/keep.md"]);
    }

    #[test]
    fn html_loses_tags_scripts_and_styles() {
        let html = "<html><STYLE>p{}</STYLE><script>var x</script><p>Fish &amp; chips</p></html>";
        let text = strip_tags(&drop_blocks(&drop_blocks(html, "script"), "style"));
        assert_eq!(text.split_whitespace().collect::<Vec<_>>(), ["Fish", "&", "chips"]);
    }

    #[test]
    fn extracts_text_html_and_docx() {
        let dir = tempfile::tempdir().unwrap();
        let txt = dir.path().join("note.txt");
        fs::write(&txt, "plain words").unwrap();
        let html = dir.path().join("page.html");
        fs::write(&html, "<p>web <b>words</b></p><script>hidden()</script>").unwrap();
        let docx = dir.path().join("letter.docx");
        let mut zip = zip::ZipWriter::new(fs::File::create(&docx).unwrap());
        zip.start_file("word/document.xml", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(b"<w:document><w:p><w:r><w:t>Dear reader</w:t></w:r></w:p></w:document>").unwrap();
        zip.finish().unwrap();

        let candidate = |path: &Path| Candidate { path: path.into(), mtime: 0, size: 1, kind: "text", cloud: false };
        let (items, thumb) = extract(&candidate(&txt), None).unwrap();
        assert_eq!(texts(&items), ["plain words"]);
        assert!(thumb.is_none());
        assert!(matches!(&items[0].input, Input::Text(t) if t == "title: note.txt | text: plain words"));

        let (items, _) = extract(&candidate(&html), None).unwrap();
        assert_eq!(items[0].text.split_whitespace().collect::<Vec<_>>(), ["web", "words"]);

        let (items, _) = extract(&candidate(&docx), None).unwrap();
        assert_eq!(texts(&items), ["Dear reader"]);
    }

    #[test]
    fn images_become_one_item_with_a_preview() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.png");
        image::RgbImage::from_pixel(1200, 600, image::Rgb([200, 30, 30])).save(&path).unwrap();

        let (items, thumb) = extract(&Candidate { path: path.clone(), mtime: 0, size: 1, kind: "image", cloud: false }, None).unwrap();
        assert_eq!(texts(&items), ["big.png"]);
        assert!(matches!(items[0].input, Input::Image(_)));
        let preview = image::load_from_memory(&thumb.unwrap()).unwrap();
        assert_eq!((preview.width(), preview.height()), (360, 180));
        assert!(!make_thumb(&path).unwrap().is_empty());
    }

    #[test]
    fn unreadable_files_are_errors_not_panics() {
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("broken.pdf");
        fs::write(&pdf, "not a pdf at all").unwrap();
        assert!(extract(&Candidate { path: pdf, mtime: 0, size: 1, kind: "pdf", cloud: false }, None).is_err());

        let img = dir.path().join("broken.jpg");
        fs::write(&img, "not an image").unwrap();
        assert!(extract(&Candidate { path: img, mtime: 0, size: 1, kind: "image", cloud: false }, None).is_err());
    }

    #[test]
    fn wildcards() {
        assert!(wildcard_match("*.tmp", "draft.tmp"));
        assert!(wildcard_match("old*", "old photos"));
        assert!(wildcard_match("a?c", "abc"));
        assert!(wildcard_match("*", ""));
        assert!(!wildcard_match("*.tmp", "draft.txt"));
        assert!(!wildcard_match("a?c", "ac"));
        assert!(wildcard_match("*a*b", "xxaxxab"));
    }

    #[test]
    fn exclusions_skip_names_patterns_and_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for path in ["keep.txt", "Archive/old.txt", "notes/draft.TMP", "photos/2019/a.txt", "photos/2020/b.txt"] {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "x").unwrap();
        }
        let rules = ["archive".to_string(), "*.tmp".into(), root.join("photos/2019").to_string_lossy().into_owned()];
        let exclusions = Exclusions::new(&rules);
        let mut found: Vec<String> = scan(root, &exclusions)
            .iter()
            .map(|c| c.path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"))
            .collect();
        found.sort();
        assert_eq!(found, ["keep.txt", "photos/2020/b.txt"]);

        assert!(exclusions.excludes(root, &root.join("Archive/new.txt")));
        assert!(exclusions.excludes(root, &root.join("sub/.git/x.txt")));
        assert!(!exclusions.excludes(root, &root.join("photos/2020/c.txt")));
        assert!(exclusions.excludes(root, Path::new("/elsewhere/x.txt")));
    }

    fn write_zip(path: &Path, parts: &[(&str, &str)]) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (name, body) in parts {
            zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn extracts_spreadsheets_and_slides() {
        let dir = tempfile::tempdir().unwrap();
        let xlsx = dir.path().join("budget.xlsx");
        write_zip(&xlsx, &[
            ("xl/sharedStrings.xml", "<sst><si><t>Rent</t></si><si><r><t>Elec</t></r><r><t>tricity</t></r></si></sst>"),
            ("xl/worksheets/sheet1.xml", concat!(
                r#"<worksheet><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1"><v>1200</v></c></row>"#,
                r#"<row r="2"><c r="A2" t="s"><v>1</v></c><c r="B2" t="inlineStr"><is><t>Q&amp;A</t></is></c><c r="C2"/></row>"#,
                "</sheetData></worksheet>")),
        ]);
        let pptx = dir.path().join("talk.pptx");
        write_zip(&pptx, &[
            ("ppt/slides/slide2.xml", "<p:sld><a:p><a:r><a:t>Second</a:t></a:r><a:tab/></a:p></p:sld>"),
            ("ppt/slides/slide10.xml", "<p:sld><a:p><a:r><a:t>Tenth</a:t></a:r></a:p></p:sld>"),
            ("ppt/slides/slide1.xml", r#"<p:sld><a:p><a:r><a:t xml:space="preserve">Hello </a:t></a:r><a:r><a:t>world</a:t></a:r></a:p></p:sld>"#),
        ]);
        let candidate = |path: &Path| Candidate { path: path.into(), mtime: 0, size: 1, kind: "text", cloud: false };

        let (items, _) = extract(&candidate(&xlsx), None).unwrap();
        assert_eq!(texts(&items), ["Rent\t1200\nElectricity\tQ&A"]);

        let (items, _) = extract(&candidate(&pptx), None).unwrap();
        let slides: Vec<_> = items.iter().map(|i| (i.page, i.text.trim())).collect();
        assert_eq!(slides, [(Some(1), "Hello world"), (Some(2), "Second"), (Some(3), "Tenth")]);
    }

    /// Reads a fixed text from every picture, and from PDF pages it is asked about.
    struct FakeOcr(&'static str);

    impl Ocr for FakeOcr {
        fn image(&self, _img: &image::RgbaImage) -> Result<String> {
            Ok(self.0.into())
        }
        fn pdf_page(&self, _path: &Path, page: usize) -> Result<String> {
            Ok(format!("{} on page {page}", self.0))
        }
    }

    #[test]
    fn ocr_adds_the_text_found_in_pictures() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("screenshot.png");
        image::RgbImage::from_pixel(50, 40, image::Rgb([255, 255, 255])).save(&path).unwrap();
        let candidate = Candidate { path, mtime: 0, size: 1, kind: "image", cloud: false };

        let (items, _) = extract(&candidate, Some(&FakeOcr("Invoice number 2024-0042 total due"))).unwrap();
        assert_eq!(texts(&items), ["screenshot.png", "Invoice number 2024-0042 total due"]);
        assert!(matches!(&items[1].input, Input::Text(t) if t.starts_with("title: screenshot.png")));

        // A few stray letters in a photo are not worth a passage.
        let (items, _) = extract(&candidate, Some(&FakeOcr("lI 1 o"))).unwrap();
        assert_eq!(items.len(), 1);
    }
}
