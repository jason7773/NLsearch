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

/// Every vector in memory, as one flat matrix, for brute-force search. Vectors are kept
/// as 8-bit integers with one scale each, a quarter of the memory of 32-bit floats; the
/// cosine they give is within about 0.002 of the exact one. The index keeps full floats.
#[derive(Default)]
pub struct VecCache {
    pub ids: Vec<i64>,
    /// The file each vector belongs to, for the search filters.
    pub files: Vec<i64>,
    pub kinds: Vec<u8>,
    pub data: Vec<i8>,
    /// Per vector: the value of one step of `data`.
    pub scales: Vec<f32>,
}

/// Below this many vectors a search scans on one thread; starting threads costs more.
const PARALLEL_MIN: usize = 16_384;

impl VecCache {
    pub fn load(conn: &Connection) -> Result<VecCache> {
        let mut cache = VecCache::default();
        let mut stmt = conn.prepare(
            "SELECT c.id, c.file_id, f.kind, c.emb FROM chunks c JOIN files f ON f.id = c.file_id",
        )?;
        let mut rows = stmt.query([])?;
        let mut vec = vec![0f32; DIM];
        while let Some(row) = rows.next()? {
            let blob = row.get_ref(3)?.as_blob()?;
            if blob.len() != DIM * 4 {
                continue;
            }
            for (x, b) in vec.iter_mut().zip(blob.chunks_exact(4)) {
                *x = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            }
            cache.push(row.get(0)?, row.get(1)?, kind_code(&row.get::<_, String>(2)?), &vec);
        }
        Ok(cache)
    }

    pub fn push(&mut self, id: i64, file_id: i64, kind: u8, vec: &[f32]) {
        self.ids.push(id);
        self.files.push(file_id);
        self.kinds.push(kind);
        let max = vec.iter().fold(0f32, |m, x| m.max(x.abs()));
        let scale = if max > 0.0 { max / 127.0 } else { 1.0 };
        self.data.extend(vec.iter().map(|x| (x / scale).round() as i8));
        self.scales.push(scale);
    }

    /// The average direction of a file's vectors, normalised: what the file is about as
    /// a whole. None when the file has no vectors.
    pub fn file_vector(&self, file_id: i64) -> Option<Vec<f32>> {
        let mut sum = vec![0f32; DIM];
        let mut found = false;
        for (i, v) in self.data.chunks_exact(DIM).enumerate() {
            if self.files[i] == file_id {
                found = true;
                let scale = self.scales[i];
                sum.iter_mut().zip(v).for_each(|(s, x)| *s += *x as f32 * scale);
            }
        }
        let norm = sum.iter().map(|x| x * x).sum::<f32>().sqrt();
        (found && norm > 0.0).then(|| sum.iter().map(|x| x / norm).collect())
    }

    /// Top `k` (chunk id, cosine) pairs among chunks whose kind and file pass `allow`.
    /// Large caches are scanned in parallel, one slice per core.
    pub fn top_k(&self, query: &[f32], k: usize, allow: impl Fn(u8, i64) -> bool + Sync) -> Vec<(i64, f32)> {
        let n = self.ids.len();
        let threads = std::thread::available_parallelism().map_or(1, |t| t.get()).min(8);
        let mut scored = if n < PARALLEL_MIN || threads < 2 {
            self.scan(0..n, query, k, &allow)
        } else {
            let per = n.div_ceil(threads);
            std::thread::scope(|scope| {
                let parts: Vec<_> = (0..n)
                    .step_by(per)
                    .map(|start| {
                        let allow = &allow;
                        scope.spawn(move || self.scan(start..(start + per).min(n), query, k, allow))
                    })
                    .collect();
                parts.into_iter().flat_map(|p| p.join().unwrap()).collect()
            })
        };
        best(&mut scored, k);
        scored.sort_unstable_by(by_score);
        scored
    }

    /// The best `k` of the vectors in `range`, unordered.
    fn scan(&self, range: std::ops::Range<usize>, query: &[f32], k: usize, allow: &impl Fn(u8, i64) -> bool) -> Vec<(i64, f32)> {
        let mut scored: Vec<(i64, f32)> = range
            .filter(|&i| allow(self.kinds[i], self.files[i]))
            .map(|i| (self.ids[i], dot(&self.data[i * DIM..(i + 1) * DIM], query) * self.scales[i]))
            .collect();
        best(&mut scored, k);
        scored
    }
}

fn by_score(a: &(i64, f32), b: &(i64, f32)) -> std::cmp::Ordering {
    b.1.total_cmp(&a.1)
}

/// Keeps the best `k`, unordered. Setting them apart is linear; sorting all would not be.
fn best(scored: &mut Vec<(i64, f32)>, k: usize) {
    if scored.len() > k && k > 0 {
        scored.select_nth_unstable_by(k - 1, by_score);
    }
    scored.truncate(k);
}

/// Dot product in eight independent lanes, which the compiler turns into SIMD; a plain
/// running sum has to add in order and cannot be vectorised.
fn dot(a: &[i8], b: &[f32]) -> f32 {
    let mut lanes = [0f32; 8];
    let (ca, cb) = (a.chunks_exact(8), b.chunks_exact(8));
    let tail: f32 = ca.remainder().iter().zip(cb.remainder()).map(|(x, y)| *x as f32 * y).sum();
    for (x, y) in ca.zip(cb) {
        for i in 0..8 {
            lanes[i] += x[i] as f32 * y[i];
        }
    }
    lanes.iter().sum::<f32>() + tail
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::normalize;
    use std::collections::{HashMap, HashSet};

    /// Unit vectors from a fixed pseudo-random sequence.
    fn vectors(n: usize, seed: u64) -> Vec<Vec<f32>> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                let mut v: Vec<f32> = (0..DIM)
                    .map(|_| {
                        s ^= s << 13;
                        s ^= s >> 7;
                        s ^= s << 17;
                        (s >> 40) as f32 / (1u64 << 24) as f32 - 0.5
                    })
                    .collect();
                normalize(&mut v);
                v
            })
            .collect()
    }

    #[test]
    fn eight_bit_scan_matches_exact_cosine() {
        // Enough vectors for the parallel path.
        let docs = vectors(PARALLEL_MIN + 1000, 7);
        let mut cache = VecCache::default();
        for (i, v) in docs.iter().enumerate() {
            cache.push(i as i64, (i / 3) as i64, 0, v);
        }
        for query in vectors(5, 99) {
            let mut exact: Vec<(i64, f32)> = docs
                .iter()
                .enumerate()
                .map(|(i, v)| (i as i64, v.iter().zip(&query).map(|(a, b)| a * b).sum()))
                .collect();
            exact.sort_by(|a, b| b.1.total_cmp(&a.1));
            let got = cache.top_k(&query, 50, |_, _| true);
            assert_eq!(got.len(), 50);
            assert!(got.windows(2).all(|w| w[0].1 >= w[1].1));
            let exact_score: HashMap<i64, f32> = exact.iter().copied().collect();
            for (id, score) in &got {
                assert!((score - exact_score[id]).abs() < 0.005, "{score} vs {}", exact_score[id]);
            }
            // The best match is found, and nearly all of the best 50.
            assert_eq!(got[0].0, exact[0].0);
            let top: HashSet<i64> = exact[..50].iter().map(|(id, _)| *id).collect();
            assert!(got.iter().filter(|(id, _)| top.contains(id)).count() >= 45);
        }
        // Filters apply on every thread.
        let odd = cache.top_k(&docs[0], 10, |_, file| file % 2 == 1);
        assert!(odd.iter().all(|(id, _)| (id / 3) % 2 == 1));
    }

    #[test]
    fn file_vector_averages_a_files_passages() {
        let mut cache = VecCache::default();
        let mut a = vec![0f32; DIM];
        a[0] = 1.0;
        let mut b = vec![0f32; DIM];
        b[1] = 1.0;
        cache.push(1, 10, 0, &a);
        cache.push(2, 10, 0, &b);
        let v = cache.file_vector(10).unwrap();
        assert!((v[0] - 0.7071).abs() < 0.001 && (v[1] - 0.7071).abs() < 0.001);
        assert!(cache.file_vector(11).is_none());
    }
}
