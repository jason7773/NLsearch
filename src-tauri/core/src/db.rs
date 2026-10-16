//! One SQLite file holds files, chunks, their vectors and the keyword index.

use std::path::Path;

use anyhow::Result;
use rusqlite::functions::FunctionFlags;
use rusqlite::{params, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::embed::{DIM, MODEL_ID};
use crate::text;

/// Version of what the keyword index stores; see `text::fts_text`. A database built
/// with another version has its keyword index rebuilt on open.
const KEYWORD_INDEX: &str = "cjk-1";

pub fn open(path: &Path) -> Result<Connection> {
    let mut conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(30))?;
    // The keyword index triggers call this, so it is registered before anything is written.
    conn.create_scalar_function(
        "fts_text",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC | FunctionFlags::SQLITE_INNOCUOUS,
        |ctx| Ok(text::fts_text(&ctx.get::<String>(0)?)),
    )?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS folders (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE);
         CREATE TABLE IF NOT EXISTS files (
             id INTEGER PRIMARY KEY,
             folder_id INTEGER NOT NULL,
             path TEXT NOT NULL UNIQUE,
             mtime INTEGER NOT NULL,
             size INTEGER NOT NULL,
             kind TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS files_folder ON files(folder_id);
         CREATE TABLE IF NOT EXISTS chunks (
             id INTEGER PRIMARY KEY,
             file_id INTEGER NOT NULL,
             page INTEGER,
             text TEXT NOT NULL,
             emb BLOB NOT NULL
         );
         CREATE INDEX IF NOT EXISTS chunks_file ON chunks(file_id);
         CREATE TABLE IF NOT EXISTS thumbs (file_id INTEGER PRIMARY KEY, jpeg BLOB NOT NULL);",
    )?;

    // Files of the folder still waiting to be indexed; NULL until its first scan.
    // Added after the first release, so older databases get it here.
    let has_pending = conn
        .prepare("SELECT 1 FROM pragma_table_info('folders') WHERE name = 'pending'")?
        .exists([])?;
    if !has_pending {
        conn.execute_batch("ALTER TABLE folders ADD COLUMN pending INTEGER")?;
    }
    // Whether a picture or PDF was read with OCR, so changing the setting redoes them.
    let has_ocr = conn.prepare("SELECT 1 FROM pragma_table_info('files') WHERE name = 'ocr'")?.exists([])?;
    if !has_ocr {
        conn.execute_batch("ALTER TABLE files ADD COLUMN ocr INTEGER NOT NULL DEFAULT 0")?;
    }

    build_keyword_index(&mut conn)?;

    // Vectors from another model or quantisation are not comparable: drop them.
    let stored: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = 'model'", [], |r| r.get(0))
        .ok();
    if stored.as_deref() != Some(MODEL_ID) {
        conn.execute_batch("DELETE FROM chunks; DELETE FROM files; DELETE FROM thumbs;")?;
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('model', ?1)",
            params![MODEL_ID],
        )?;
    }
    Ok(conn)
}

fn keyword_index_current(conn: &Connection) -> bool {
    conn.query_row("SELECT value FROM meta WHERE key = 'keyword_index'", [], |r| r.get::<_, String>(0))
        .is_ok_and(|v| v == KEYWORD_INDEX)
}

/// Creates the keyword index, or replaces one built by an earlier version.
///
/// The index holds `text::fts_text` of each passage rather than the passage itself, so
/// it is contentless: it keeps only tokens and positions, and rows are deleted by id.
fn build_keyword_index(conn: &mut Connection) -> Result<()> {
    if keyword_index_current(conn) {
        return Ok(());
    }
    // Several connections open at start-up; the first one in does the work.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if keyword_index_current(&tx) {
        return Ok(());
    }
    tx.execute_batch(
        "DROP TRIGGER IF EXISTS chunks_ai;
         DROP TRIGGER IF EXISTS chunks_ad;
         DROP TABLE IF EXISTS chunks_fts;

         -- Text files used to be read as UTF-8 only: Big5, GBK and other legacy files came
         -- out garbled, and UTF-16 files were taken for binary and recorded empty. Forget
         -- those so the next pass reads them again.
         CREATE TEMP TABLE redo AS
             SELECT f.id FROM files f
             WHERE f.kind = 'text' AND (
                 NOT EXISTS (SELECT 1 FROM chunks c WHERE c.file_id = f.id)
                 OR EXISTS (SELECT 1 FROM chunks c WHERE c.file_id = f.id
                            AND instr(c.text, char(65533)) > 0));
         DELETE FROM chunks WHERE file_id IN (SELECT id FROM redo);
         DELETE FROM files WHERE id IN (SELECT id FROM redo);
         DROP TABLE redo;

         CREATE VIRTUAL TABLE chunks_fts USING fts5(text, content='', contentless_delete=1);
         CREATE TRIGGER chunks_ai AFTER INSERT ON chunks BEGIN
             INSERT INTO chunks_fts(rowid, text) VALUES (new.id, fts_text(new.text));
         END;
         CREATE TRIGGER chunks_ad AFTER DELETE ON chunks BEGIN
             DELETE FROM chunks_fts WHERE rowid = old.id;
         END;
         INSERT INTO chunks_fts(rowid, text) SELECT id, fts_text(text) FROM chunks;",
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES ('keyword_index', ?1)",
        params![KEYWORD_INDEX],
    )?;
    tx.commit()?;
    Ok(())
}

/// What the user chose in Settings that changes what gets indexed. Kept in the index
/// itself, so a portable copy carries it along.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    /// Rules for `extract::Exclusions`.
    pub exclude: Vec<String>,
    /// Read text in pictures and scanned PDFs.
    pub ocr: bool,
}

impl Settings {
    pub fn load(conn: &Connection) -> Result<Settings> {
        let stored: Option<String> =
            conn.query_row("SELECT value FROM meta WHERE key = 'settings'", [], |r| r.get(0)).ok();
        Ok(stored.and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default())
    }

    pub fn save(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('settings', ?1)",
            params![serde_json::to_string(self)?],
        )?;
        Ok(())
    }
}

pub fn delete_file(conn: &Connection, file_id: i64) -> Result<()> {
    conn.execute("DELETE FROM chunks WHERE file_id = ?1", params![file_id])?;
    conn.execute("DELETE FROM thumbs WHERE file_id = ?1", params![file_id])?;
    conn.execute("DELETE FROM files WHERE id = ?1", params![file_id])?;
    Ok(())
}

pub fn delete_folder(conn: &Connection, folder_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM chunks WHERE file_id IN (SELECT id FROM files WHERE folder_id = ?1)",
        params![folder_id],
    )?;
    conn.execute(
        "DELETE FROM thumbs WHERE file_id IN (SELECT id FROM files WHERE folder_id = ?1)",
        params![folder_id],
    )?;
    conn.execute("DELETE FROM files WHERE folder_id = ?1", params![folder_id])?;
    conn.execute("DELETE FROM folders WHERE id = ?1", params![folder_id])?;
    Ok(())
}

pub fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub const KIND_TEXT: u8 = 0;
pub const KIND_PDF: u8 = 1;
pub const KIND_IMAGE: u8 = 2;

pub fn kind_code(kind: &str) -> u8 {
    match kind {
        "pdf" => KIND_PDF,
        "image" => KIND_IMAGE,
        _ => KIND_TEXT,
    }
}

/// Every vector in memory, as one flat matrix, for brute-force search.
#[derive(Default)]
pub struct VecCache {
    pub ids: Vec<i64>,
    /// The file each vector belongs to, for the search filters.
    pub files: Vec<i64>,
    pub kinds: Vec<u8>,
    pub data: Vec<f32>,
}

impl VecCache {
    pub fn load(conn: &Connection) -> Result<VecCache> {
        let mut cache = VecCache::default();
        let mut stmt = conn.prepare(
            "SELECT c.id, c.file_id, f.kind, c.emb FROM chunks c JOIN files f ON f.id = c.file_id",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let blob = row.get_ref(3)?.as_blob()?;
            if blob.len() != DIM * 4 {
                continue;
            }
            cache.ids.push(row.get(0)?);
            cache.files.push(row.get(1)?);
            cache.kinds.push(kind_code(&row.get::<_, String>(2)?));
            cache.data.extend(
                blob.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            );
        }
        Ok(cache)
    }

    pub fn push(&mut self, id: i64, file_id: i64, kind: u8, vec: &[f32]) {
        self.ids.push(id);
        self.files.push(file_id);
        self.kinds.push(kind);
        self.data.extend_from_slice(vec);
    }

    /// Top `k` (chunk id, cosine) pairs among chunks whose kind and file pass `allow`.
    pub fn top_k(&self, query: &[f32], k: usize, allow: impl Fn(u8, i64) -> bool) -> Vec<(i64, f32)> {
        let mut scored: Vec<(i64, f32)> = self
            .data
            .chunks_exact(DIM)
            .enumerate()
            .filter(|(i, _)| allow(self.kinds[*i], self.files[*i]))
            .map(|(i, v)| (self.ids[i], v.iter().zip(query).map(|(a, b)| a * b).sum()))
            .collect();
        scored.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(k);
        scored
    }
}

/// A folder path in comparable form: no trailing separator, and lower case on
/// Windows, where paths are case-insensitive.
fn comparable(path: &str) -> std::path::PathBuf {
    let trimmed = path.trim_end_matches(['\\', '/']);
    if cfg!(windows) {
        trimmed.to_lowercase().into()
    } else {
        trimmed.into()
    }
}

/// An already indexed folder that is `path` itself or contains it.
pub fn covering_folder(conn: &Connection, path: &str) -> Result<Option<String>> {
    let target = comparable(path);
    let mut stmt = conn.prepare("SELECT path FROM folders")?;
    let folders = stmt.query_map([], |r| r.get::<_, String>(0))?;
    for folder in folders {
        let folder = folder?;
        if target.starts_with(comparable(&folder)) {
            return Ok(Some(folder));
        }
    }
    Ok(None)
}

/// Folds every folder that lies inside another indexed folder into that one, so each
/// file belongs to exactly one folder. Its files move over as they are, without being
/// indexed again.
pub fn merge_nested_folders(conn: &Connection) -> Result<()> {
    let mut folders: Vec<(i64, String)> = conn
        .prepare("SELECT id, path FROM folders")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    // Shortest first, so a folder is merged into its outermost ancestor.
    folders.sort_by_key(|(id, path)| (comparable(path).components().count(), *id));

    let mut kept: Vec<(i64, std::path::PathBuf)> = Vec::new();
    for (id, path) in folders {
        let path = comparable(&path);
        match kept.iter().find(|(_, outer)| path.starts_with(outer)) {
            Some((outer_id, _)) => {
                conn.execute(
                    "UPDATE files SET folder_id = ?1 WHERE folder_id = ?2",
                    params![outer_id, id],
                )?;
                conn.execute("DELETE FROM folders WHERE id = ?1", params![id])?;
            }
            None => kept.push((id, path)),
        }
    }
    Ok(())
}
