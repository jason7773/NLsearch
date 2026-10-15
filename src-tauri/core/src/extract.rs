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

const MAX_TEXT_BYTES: u64 = 5 * 1024 * 1024;
const MAX_PDF_BYTES: u64 = 100 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 50 * 1024 * 1024;

const SKIP_DIRS: &[&str] = &[
    "node_modules", "target", "__pycache__", "$RECYCLE.BIN", "System Volume Information",
    "AppData", "venv", "dist", "build",
];

pub fn kind_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "txt" | "md" | "markdown" | "rst" | "log" | "csv" | "html" | "htm" | "docx" => "text",
        "pdf" => "pdf",
        "jpg" | "jpeg" | "png" | "webp" | "bmp" | "gif" => "image",
        _ => return None,
    })
}

pub struct Candidate {
    pub path: PathBuf,
    pub mtime: i64,
    pub size: u64,
    pub kind: &'static str,
}

pub struct Item {
    pub page: Option<i64>,
    /// Stored for snippets and keyword search.
    pub text: String,
    pub input: Input,
}

pub fn scan(root: &Path) -> Vec<Candidate> {
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
pub fn extract(file: &Candidate) -> Result<(Vec<Item>, Option<Vec<u8>>)> {
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
        let mut found: Vec<String> = scan(root)
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

        let candidate = |path: &Path| Candidate { path: path.into(), mtime: 0, size: 1, kind: "text" };
        let (items, thumb) = extract(&candidate(&txt)).unwrap();
        assert_eq!(texts(&items), ["plain words"]);
        assert!(thumb.is_none());
        assert!(matches!(&items[0].input, Input::Text(t) if t == "title: note.txt | text: plain words"));

        let (items, _) = extract(&candidate(&html)).unwrap();
        assert_eq!(items[0].text.split_whitespace().collect::<Vec<_>>(), ["web", "words"]);

        let (items, _) = extract(&candidate(&docx)).unwrap();
        assert_eq!(texts(&items), ["Dear reader"]);
    }

    #[test]
    fn images_become_one_item_with_a_preview() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.png");
        image::RgbImage::from_pixel(1200, 600, image::Rgb([200, 30, 30])).save(&path).unwrap();

        let (items, thumb) = extract(&Candidate { path: path.clone(), mtime: 0, size: 1, kind: "image" }).unwrap();
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
        assert!(extract(&Candidate { path: pdf, mtime: 0, size: 1, kind: "pdf" }).is_err());

        let img = dir.path().join("broken.jpg");
        fs::write(&img, "not an image").unwrap();
        assert!(extract(&Candidate { path: img, mtime: 0, size: 1, kind: "image" }).is_err());
    }
}
