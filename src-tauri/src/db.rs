//! One SQLite file holds files, chunks, their vectors and the keyword index.

use std::path::Path;

use anyhow::Result;
use rusqlite::{params, Connection};

use crate::engine::{DIM, MODEL_ID};

pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(30))?;
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
         CREATE TABLE IF NOT EXISTS thumbs (file_id INTEGER PRIMARY KEY, jpeg BLOB NOT NULL);
         CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts
             USING fts5(text, content='chunks', content_rowid='id');
         CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
             INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
         END;
         CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
             INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES ('delete', old.id, old.text);
         END;",
    )?;

    // Files of the folder still waiting to be indexed; NULL until its first scan.
    // Added after the first release, so older databases get it here.
    let has_pending = conn
        .prepare("SELECT 1 FROM pragma_table_info('folders') WHERE name = 'pending'")?
        .exists([])?;
    if !has_pending {
        conn.execute_batch("ALTER TABLE folders ADD COLUMN pending INTEGER")?;
    }

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
    pub kinds: Vec<u8>,
    pub data: Vec<f32>,
}

impl VecCache {
    pub fn load(conn: &Connection) -> Result<VecCache> {
        let mut cache = VecCache::default();
        let mut stmt = conn.prepare(
            "SELECT c.id, f.kind, c.emb FROM chunks c JOIN files f ON f.id = c.file_id",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let blob = row.get_ref(2)?.as_blob()?;
            if blob.len() != DIM * 4 {
                continue;
            }
            cache.ids.push(row.get(0)?);
            cache.kinds.push(kind_code(&row.get::<_, String>(1)?));
            cache.data.extend(
                blob.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            );
        }
        Ok(cache)
    }

    pub fn push(&mut self, id: i64, kind: u8, vec: &[f32]) {
        self.ids.push(id);
        self.kinds.push(kind);
        self.data.extend_from_slice(vec);
    }

    /// Top `k` (chunk id, cosine) pairs among chunks whose kind passes `allow`.
    pub fn top_k(&self, query: &[f32], k: usize, allow: impl Fn(u8) -> bool) -> Vec<(i64, f32)> {
        let mut scored: Vec<(i64, f32)> = self
            .data
            .chunks_exact(DIM)
            .enumerate()
            .filter(|(i, _)| allow(self.kinds[*i]))
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
