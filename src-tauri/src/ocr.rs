//! Windows' built-in OCR (Windows.Media.Ocr), which reads text in the languages
//! installed in Windows. Scanned PDF pages are rendered with Windows.Data.Pdf first.
//! Nothing is downloaded; elsewhere there is no OCR.

use std::path::Path;

use anyhow::Result;
use nlsearch_core::extract::Ocr;

/// The languages Windows can read text in, by display name. Empty when OCR is unavailable.
pub fn languages() -> Vec<String> {
    imp::languages().unwrap_or_default()
}

/// The OCR engine for the languages in the user's Windows profile, if Windows has one.
pub fn engine() -> Option<WindowsOcr> {
    imp::available().then_some(WindowsOcr)
}

pub struct WindowsOcr;

impl Ocr for WindowsOcr {
    fn image(&self, img: &nlsearch_core::image::RgbaImage) -> Result<String> {
        imp::image(img)
    }

    fn pdf_page(&self, path: &Path, page: usize) -> Result<String> {
        imp::pdf_page(path, page)
    }
}

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result};
    use nlsearch_core::text::join_words;
    use windows::core::HSTRING;
    use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
    use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Security::Cryptography::CryptographicBuffer;
    use windows::Storage::StorageFile;
    use windows::Storage::Streams::InMemoryRandomAccessStream;

    /// Longest side a scanned page is rendered at, in pixels.
    const PAGE_SIDE: f32 = 2400.0;

    thread_local! {
        // Each indexing thread keeps its own engine, and the PDF it is reading.
        static ENGINE: RefCell<Option<OcrEngine>> = const { RefCell::new(None) };
        static PDF: RefCell<Option<(PathBuf, PdfDocument)>> = const { RefCell::new(None) };
    }

    pub fn languages() -> Result<Vec<String>> {
        let list = OcrEngine::AvailableRecognizerLanguages()?;
        (0..list.Size()?).map(|i| Ok(list.GetAt(i)?.DisplayName()?.to_string_lossy())).collect()
    }

    pub fn available() -> bool {
        OcrEngine::TryCreateFromUserProfileLanguages().is_ok()
    }

    fn recognize(bitmap: &SoftwareBitmap) -> Result<String> {
        let engine = ENGINE.with(|slot| -> Result<OcrEngine> {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                *slot = Some(OcrEngine::TryCreateFromUserProfileLanguages().context("no OCR language installed")?);
            }
            Ok(slot.clone().unwrap())
        })?;
        let result = engine.RecognizeAsync(bitmap)?.join()?;
        let lines = result.Lines()?;
        let mut text = String::new();
        for i in 0..lines.Size()? {
            let words = lines.GetAt(i)?.Words()?;
            let words: Vec<String> =
                (0..words.Size()?).map(|j| Ok(words.GetAt(j)?.Text()?.to_string_lossy())).collect::<Result<_>>()?;
            text.push_str(&join_words(&words));
            text.push('\n');
        }
        Ok(text)
    }

    pub fn image(img: &nlsearch_core::image::RgbaImage) -> Result<String> {
        // Windows wants blue, green, red, alpha.
        let mut bgra = img.as_raw().clone();
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let buffer = CryptographicBuffer::CreateFromByteArray(&bgra)?;
        let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            img.width() as i32,
            img.height() as i32,
            BitmapAlphaMode::Premultiplied,
        )?;
        recognize(&bitmap)
    }

    pub fn pdf_page(path: &Path, page: usize) -> Result<String> {
        let doc = PDF.with(|slot| -> Result<PdfDocument> {
            let mut slot = slot.borrow_mut();
            if let Some((open, doc)) = slot.as_ref() {
                if open == path {
                    return Ok(doc.clone());
                }
            }
            let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path.as_os_str()))?.join()?;
            let doc = PdfDocument::LoadFromFileAsync(&file)?.join()?;
            *slot = Some((path.to_path_buf(), doc.clone()));
            Ok(doc)
        })?;
        let pdf_page = doc.GetPage(page as u32)?;
        let size = pdf_page.Size()?;
        let scale = PAGE_SIDE / size.Width.max(size.Height).max(1.0);
        let options = PdfPageRenderOptions::new()?;
        options.SetDestinationWidth((size.Width * scale) as u32)?;
        options.SetDestinationHeight((size.Height * scale) as u32)?;
        let stream = InMemoryRandomAccessStream::new()?;
        pdf_page.RenderWithOptionsToStreamAsync(&stream, &options)?.join()?;
        let bitmap = BitmapDecoder::CreateAsync(&stream)?.join()?.GetSoftwareBitmapAsync()?.join()?;
        recognize(&bitmap)
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::Path;

    use anyhow::{bail, Result};

    pub fn languages() -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    pub fn available() -> bool {
        false
    }

    pub fn image(_img: &nlsearch_core::image::RgbaImage) -> Result<String> {
        bail!("OCR needs Windows")
    }

    pub fn pdf_page(_path: &Path, _page: usize) -> Result<String> {
        bail!("OCR needs Windows")
    }
}
