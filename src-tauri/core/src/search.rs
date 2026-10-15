//! Combines the semantic and keyword rankings into the result list.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::Serialize;

use crate::db;
use crate::embed::Input;
use crate::text;

/// Semantic candidates taken from the vector cache before fusion.
pub const SEMANTIC_CANDIDATES: usize = 200;

#[derive(Serialize)]
pub struct Hit {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub page: Option<i64>,
    pub snippet: String,
    /// Cosine similarity; absent for hits found by keyword only.
    pub score: Option<f32>,
    pub keyword: bool,
    /// Key of the stored preview, for images: pass it to the `thumb` protocol.
    pub thumb: Option<String>,
}

/// What is sent to the model for a query.
pub fn query_input(query: &str) -> Input {
    Input::Text(format!("task: search result | query: {query}"))
}

/// Which kinds of chunk the result filter lets through: "docs", "images" or anything
/// else for all.
pub fn allows(filter: &str, kind: u8) -> bool {
    match filter {
        "docs" => kind != db::KIND_IMAGE,
        "images" => kind == db::KIND_IMAGE,
        _ => true,
    }
}

/// Fuses `semantic` (chunk id and cosine, best first) with a keyword search for
/// `query`, and returns the best passage of each of the top `limit` files.
pub fn rank(
    conn: &Connection,
    semantic: &[(i64, f32)],
    query: &str,
    filter: &str,
    limit: usize,
) -> Result<Vec<Hit>> {
    // Exact-match side: every query word must appear in the passage.
    let fts_query = text::fts_query(query);
    let mut keyword: Vec<i64> = Vec::new();
    if !fts_query.is_empty() {
        let mut stmt = conn.prepare(
            "SELECT c.id, f.kind FROM chunks_fts
             JOIN chunks c ON c.id = chunks_fts.rowid
             JOIN files f ON f.id = c.file_id
             WHERE chunks_fts MATCH ?1 ORDER BY bm25(chunks_fts) LIMIT 100",
        )?;
        let rows = stmt.query_map(params![fts_query], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (id, kind) = row?;
            if allows(filter, db::kind_code(&kind)) {
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
    // Ties go to the lower chunk id, so equal scores come out the same every time.
    ranked.sort_unstable_by(|a, b| b.1 .0.total_cmp(&a.1 .0).then(a.0.cmp(&b.0)));

    // Best passage per file.
    let mut stmt = conn.prepare(
        "SELECT f.path, f.kind, c.page, c.text, f.id, f.mtime FROM chunks c
         JOIN files f ON f.id = c.file_id WHERE c.id = ?1",
    )?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::VecCache;
    use crate::embed::Embedder;
    use crate::extract::{extract, scan};
    use crate::index::{embed_items, plan_folder, tidy, write_file};
    use crate::testing::FakeEmbedder;
    use std::path::Path;

    /// Runs one indexing pass over every folder, the way the app does, minus threads.
    fn index_all(conn: &Connection, embedder: &dyn Embedder, cache: &mut VecCache) -> usize {
        tidy(conn).unwrap();
        let folders: Vec<(i64, String)> = conn
            .prepare("SELECT id, path FROM folders")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let mut indexed = 0;
        for (folder_id, path) in folders {
            for file in plan_folder(conn, folder_id, scan(Path::new(&path))).unwrap() {
                let (items, thumb) = extract(&file).unwrap_or_default();
                let rows = embed_items(embedder, items, || true).unwrap();
                for (id, vec) in write_file(conn, folder_id, &file, rows, thumb.as_deref()).unwrap() {
                    cache.push(id, db::kind_code(file.kind), &vec);
                }
                indexed += 1;
            }
        }
        indexed
    }

    fn search(conn: &Connection, cache: &VecCache, query: &str, filter: &str) -> Vec<Hit> {
        let qvec = FakeEmbedder::new().embed(&[query_input(query)]).unwrap().remove(0);
        let semantic = cache.top_k(&qvec, SEMANTIC_CANDIDATES, |k| allows(filter, k));
        rank(conn, &semantic, query, filter, 10).unwrap()
    }

    fn names(hits: &[Hit]) -> Vec<&str> {
        hits.iter().map(|h| h.name.as_str()).collect()
    }

    #[test]
    fn indexes_a_folder_and_finds_files() {
        let dir = tempfile::tempdir().unwrap();
        let docs = dir.path().join("docs");
        std::fs::create_dir_all(docs.join("photos")).unwrap();
        std::fs::write(docs.join("cake.txt"), "Chocolate cake recipe with butter and sugar").unwrap();
        std::fs::write(docs.join("taxes.md"), "Annual tax return checklist and receipts").unwrap();
        std::fs::write(docs.join("weather.txt"), "今天颱風來襲，學校停課").unwrap();
        image::RgbImage::from_pixel(40, 30, image::Rgb([0, 90, 200]))
            .save(docs.join("photos/sea.png"))
            .unwrap();

        let conn = db::open(&dir.path().join("index.db")).unwrap();
        conn.execute("INSERT INTO folders (path) VALUES (?1)", params![docs.to_string_lossy()]).unwrap();
        let embedder = FakeEmbedder::new();
        let mut cache = VecCache::default();
        assert_eq!(index_all(&conn, &embedder, &mut cache), 4);

        // Semantic side: the fake model matches shared words.
        let hits = search(&conn, &cache, "chocolate recipe", "all");
        assert_eq!(hits[0].name, "cake.txt");
        assert!(hits[0].score.unwrap() > 0.3);
        assert!(hits[0].snippet.starts_with("Chocolate cake"));

        // Keyword side: Chinese words found inside a sentence.
        let hits = search(&conn, &cache, "颱風", "all");
        assert_eq!(hits[0].name, "weather.txt");
        assert!(hits[0].keyword);

        // Filters.
        let images = search(&conn, &cache, "sea", "images");
        assert_eq!(names(&images), ["sea.png"]);
        assert!(images[0].thumb.is_some());
        assert!(names(&search(&conn, &cache, "sea", "docs")).iter().all(|n| *n != "sea.png"));

        // The cache built while indexing matches what a reload from disk gives.
        let loaded = VecCache::load(&conn).unwrap();
        assert_eq!(loaded.ids, cache.ids);
        assert_eq!(loaded.data, cache.data);

        // A second pass skips unchanged files and picks up an edit.
        assert_eq!(index_all(&conn, &embedder, &mut cache), 0);
        std::fs::write(docs.join("taxes.md"), "Tax return deadline moved, plus chocolate").unwrap();
        let file = std::fs::File::options().write(true).open(docs.join("taxes.md")).unwrap();
        file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(index_all(&conn, &embedder, &mut cache), 1);
        let cache = VecCache::load(&conn).unwrap();
        assert!(names(&search(&conn, &cache, "deadline", "all")).contains(&"taxes.md"));
        assert!(search(&conn, &cache, "receipts", "all").iter().all(|h| !h.keyword));
    }

    #[test]
    fn one_hit_per_file_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open(&dir.path().join("index.db")).unwrap();
        conn.execute("INSERT INTO folders (id, path) VALUES (1, '/d')", []).unwrap();
        let mut semantic = Vec::new();
        for f in 0..5 {
            let file = crate::extract::Candidate { path: format!("/d/{f}.txt").into(), mtime: 0, size: 1, kind: "text" };
            let rows = (0..3)
                .map(|p| crate::index::Row { page: Some(p), text: format!("file {f} page {p}"), vec: FakeEmbedder::vector("x") })
                .collect();
            for (id, _) in write_file(&conn, 1, &file, rows, None).unwrap() {
                semantic.push((id, 0.9));
            }
        }
        let hits = rank(&conn, &semantic, "", "all", 3).unwrap();
        assert_eq!(names(&hits), ["0.txt", "1.txt", "2.txt"]);
        assert_eq!(hits[0].page, Some(0));
    }
}
