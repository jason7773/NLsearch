//! Combines the semantic and keyword rankings into the result list.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

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
    /// Every query word occurs in the passage as typed: shown as an exact match.
    pub keyword: bool,
    /// Key of the stored preview, for images: pass it to the `thumb` protocol.
    pub thumb: Option<String>,
    /// Last modified, in seconds since 1970.
    pub modified: i64,
}

/// Semantic candidates for a list of `limit` files: enough that files with many matching
/// passages do not crowd the rest out when more results are asked for.
pub fn candidates(limit: usize) -> usize {
    SEMANTIC_CANDIDATES.max(limit * 4)
}

/// Narrows a search beyond the kind tabs. Empty fields do not filter.
#[derive(Default, Deserialize)]
pub struct Filters {
    /// Only files modified at or after this time, in seconds since 1970.
    pub since: Option<i64>,
    /// Only files in this indexed folder.
    pub folder: Option<i64>,
    /// Only files with one of these extensions, lower case and without the dot.
    #[serde(default)]
    pub extensions: Vec<String>,
}

/// The ids of the files that pass `filters`, or None when nothing is filtered.
pub fn allowed_files(conn: &Connection, filters: &Filters) -> Result<Option<HashSet<i64>>> {
    if filters.since.is_none() && filters.folder.is_none() && filters.extensions.is_empty() {
        return Ok(None);
    }
    let mut stmt = conn.prepare("SELECT id, folder_id, mtime, path FROM files")?;
    let mut rows = stmt.query([])?;
    let mut allowed = HashSet::new();
    while let Some(row) = rows.next()? {
        let (folder, mtime): (i64, i64) = (row.get(1)?, row.get(2)?);
        if filters.folder.is_some_and(|f| f != folder) || filters.since.is_some_and(|s| mtime < s) {
            continue;
        }
        if !filters.extensions.is_empty() {
            let path: String = row.get(3)?;
            let ext = std::path::Path::new(&path)
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if !filters.extensions.contains(&ext) {
                continue;
            }
        }
        allowed.insert(row.get(0)?);
    }
    Ok(Some(allowed))
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
/// `query`, and returns the best passage of each of the top `limit` files. `allowed`
/// limits the keyword side to those files, as `semantic` already is.
pub fn rank(
    conn: &Connection,
    semantic: &[(i64, f32)],
    query: &str,
    filter: &str,
    allowed: Option<&HashSet<i64>>,
    limit: usize,
) -> Result<Vec<Hit>> {
    // Keyword side, ranked by bm25 over the query's words and character pairs, so a
    // question finds passages that share its key words without containing it whole.
    // Passages holding every query word, as typed, are marked as exact matches.
    let exact = keyword_ids(conn, &text::fts_query(query), filter, allowed)?;
    let candidates: HashSet<i64> = semantic.iter().map(|(id, _)| *id).collect();
    let keyword: Vec<i64> = keyword_ids(conn, &text::fts_query_any(query), filter, allowed)?
        .into_iter()
        // A passage that only shares a few characters, and that the model did not pick
        // either, is noise: it would show without a similarity score.
        .filter(|id| semantic.is_empty() || candidates.contains(id) || exact.contains(id))
        .take(100)
        .collect();
    let exact: HashSet<i64> = exact.into_iter().collect();

    // Reciprocal rank fusion of the two rankings.
    let rrf = |rank: usize| 1.0 / (60.0 + rank as f64);
    let mut fused: HashMap<i64, (f64, Option<f32>, bool)> = HashMap::new();
    for (rank, (id, score)) in semantic.iter().enumerate() {
        fused.insert(*id, (rrf(rank), Some(*score), false));
    }
    for (rank, id) in keyword.iter().enumerate() {
        let entry = fused.entry(*id).or_insert((0.0, None, false));
        entry.0 += rrf(rank);
        entry.2 = exact.contains(id);
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
        // An image's own passage is just its name; text read from it by OCR is shown.
        let snippet = if kind == "image" && text == name {
            String::new()
        } else {
            text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(260).collect()
        };
        // The modification time makes the key change when the file does, so previews can be cached.
        let thumb = (kind == "image").then(|| format!("{file_id}-{mtime}"));
        hits.push(Hit { path, name, kind, page, snippet, score, keyword, thumb, modified: mtime });
    }
    Ok(hits)
}

/// Chunk ids matching an FTS5 query, best first, among those `filter` and `allowed`
/// let through. Empty for an empty query.
fn keyword_ids(conn: &Connection, fts_query: &str, filter: &str, allowed: Option<&HashSet<i64>>) -> Result<Vec<i64>> {
    let mut ids = Vec::new();
    if fts_query.is_empty() {
        return Ok(ids);
    }
    let mut stmt = conn.prepare_cached(
        "SELECT c.id, f.kind, f.id FROM chunks_fts
         JOIN chunks c ON c.id = chunks_fts.rowid
         JOIN files f ON f.id = c.file_id
         WHERE chunks_fts MATCH ?1 ORDER BY bm25(chunks_fts) LIMIT ?2",
    )?;
    // Filters throw many matches away, so look further down the list.
    let depth = if allowed.is_some() { 2000 } else { 400 };
    let rows = stmt.query_map(params![fts_query, depth], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
    })?;
    for row in rows {
        let (id, kind, file_id) = row?;
        if allows(filter, db::kind_code(&kind)) && allowed.map_or(true, |a| a.contains(&file_id)) {
            ids.push(id);
        }
    }
    Ok(ids)
}

/// Files whose content is closest to the file at `path` as a whole, best first, leaving
/// that file out. Empty when the file is not indexed.
pub fn similar(
    conn: &Connection,
    cache: &db::VecCache,
    path: &str,
    filter: &str,
    allowed: Option<&HashSet<i64>>,
    limit: usize,
) -> Result<Vec<Hit>> {
    let file_id: Option<i64> = conn
        .query_row("SELECT id FROM files WHERE path = ?1", params![path], |r| r.get(0))
        .ok();
    let Some(file_id) = file_id else { return Ok(Vec::new()) };
    let Some(vector) = cache.file_vector(file_id) else { return Ok(Vec::new()) };
    let semantic = cache.top_k(&vector, candidates(limit), |kind, file| {
        file != file_id && allows(filter, kind) && allowed.map_or(true, |a| a.contains(&file))
    });
    // An empty query leaves out the keyword side.
    rank(conn, &semantic, "", filter, allowed, limit)
}

/// Text of one file, page by page, for the preview pane.
#[derive(Serialize)]
pub struct Preview {
    pub kind: String,
    pub sections: Vec<Section>,
    /// Text was left out to keep the preview light.
    pub truncated: bool,
}

#[derive(Serialize)]
pub struct Section {
    /// Page or slide number, where the file has them.
    pub page: Option<i64>,
    pub text: String,
}

/// How much text a preview carries at most.
pub const PREVIEW_CHARS: usize = 40_000;

/// The indexed text of the file at `path`, with the overlap between passages removed.
pub fn preview(conn: &Connection, path: &str) -> Result<Preview> {
    let (file_id, kind): (i64, String) =
        conn.query_row("SELECT id, kind FROM files WHERE path = ?1", params![path], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut stmt = conn.prepare("SELECT page, text FROM chunks WHERE file_id = ?1 ORDER BY id")?;
    let rows = stmt.query_map(params![file_id], |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, String>(1)?)))?;

    // Consecutive passages of the same page belong together.
    let mut pages: Vec<(Option<i64>, Vec<String>)> = Vec::new();
    for row in rows {
        let (page, text) = row?;
        // A picture's own passage is just its name.
        if kind == "image" && text == name {
            continue;
        }
        match pages.last_mut() {
            Some((p, texts)) if *p == page => texts.push(text),
            _ => pages.push((page, vec![text])),
        }
    }

    let mut sections = Vec::new();
    let mut budget = PREVIEW_CHARS;
    let mut truncated = false;
    for (page, texts) in pages {
        if budget == 0 {
            truncated = true;
            break;
        }
        let text = text::join_passages(texts.iter().map(String::as_str));
        let count = text.chars().count();
        let text = if count > budget {
            truncated = true;
            text.chars().take(budget).collect()
        } else {
            text
        };
        budget = budget.saturating_sub(count);
        sections.push(Section { page, text });
    }
    Ok(Preview { kind, sections, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::VecCache;
    use crate::embed::Embedder;
    use crate::extract::{extract, scan, Exclusions};
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
            let found = scan(Path::new(&path), &Exclusions::default());
            for file in plan_folder(conn, folder_id, found, false, false).unwrap() {
                let (items, thumb) = extract(&file, None).unwrap_or_default();
                let rows = embed_items(embedder, items, || true).unwrap();
                let (file_id, added) = write_file(conn, folder_id, &file, rows, thumb.as_deref(), false).unwrap();
                for (id, vec) in added {
                    cache.push(id, file_id, db::kind_code(file.kind), &vec);
                }
                indexed += 1;
            }
        }
        indexed
    }

    fn search(conn: &Connection, cache: &VecCache, query: &str, filter: &str) -> Vec<Hit> {
        search_filtered(conn, cache, query, filter, &Filters::default())
    }

    fn search_filtered(conn: &Connection, cache: &VecCache, query: &str, filter: &str, filters: &Filters) -> Vec<Hit> {
        let qvec = FakeEmbedder::new().embed(&[query_input(query)]).unwrap().remove(0);
        let allowed = allowed_files(conn, filters).unwrap();
        let semantic = cache.top_k(&qvec, SEMANTIC_CANDIDATES, |k, f| {
            allows(filter, k) && allowed.as_ref().map_or(true, |a| a.contains(&f))
        });
        rank(conn, &semantic, query, filter, allowed.as_ref(), 10).unwrap()
    }

    fn hits_have_dates(hits: &[Hit]) -> bool {
        !hits.is_empty() && hits.iter().all(|h| h.modified > 0)
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

        // Extension, folder and date filters, on both the semantic and keyword sides.
        let md_only = Filters { extensions: vec!["md".into()], ..Default::default() };
        assert_eq!(names(&search_filtered(&conn, &cache, "chocolate recipe", "all", &md_only)), ["taxes.md"]);
        assert!(search_filtered(&conn, &cache, "颱風", "all", &md_only).iter().all(|h| h.name == "taxes.md" && !h.keyword));
        let future = Filters { since: Some(i64::MAX), ..Default::default() };
        assert!(search_filtered(&conn, &cache, "chocolate", "all", &future).is_empty());
        let other_folder = Filters { folder: Some(999), ..Default::default() };
        assert!(search_filtered(&conn, &cache, "chocolate", "all", &other_folder).is_empty());
        assert!(hits_have_dates(&search(&conn, &cache, "chocolate", "all")));

        // The cache built while indexing matches what a reload from disk gives.
        let loaded = VecCache::load(&conn).unwrap();
        assert_eq!(loaded.ids, cache.ids);
        assert_eq!(loaded.files, cache.files);
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
    fn finds_similar_files_and_previews_text() {
        let dir = tempfile::tempdir().unwrap();
        let docs = dir.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("cake.txt"), "Chocolate cake recipe with butter and sugar").unwrap();
        std::fs::write(docs.join("brownies.txt"), "Chocolate brownies recipe with butter").unwrap();
        std::fs::write(docs.join("taxes.md"), "Annual tax return checklist and receipts").unwrap();
        let long: String = (0..900).map(|i| format!("word{i} ")).collect();
        std::fs::write(docs.join("long.txt"), &long).unwrap();

        let conn = db::open(&dir.path().join("index.db")).unwrap();
        conn.execute("INSERT INTO folders (path) VALUES (?1)", params![docs.to_string_lossy()]).unwrap();
        let mut cache = VecCache::default();
        index_all(&conn, &FakeEmbedder::new(), &mut cache);

        let cake = docs.join("cake.txt").to_string_lossy().into_owned();
        let hits = similar(&conn, &cache, &cake, "all", None, 10).unwrap();
        assert_eq!(hits[0].name, "brownies.txt");
        assert!(hits.iter().all(|h| h.name != "cake.txt"));
        assert!(similar(&conn, &cache, "/not/indexed.txt", "all", None, 10).unwrap().is_empty());

        let p = preview(&conn, &docs.join("long.txt").to_string_lossy()).unwrap();
        assert_eq!(p.kind, "text");
        assert_eq!(p.sections.len(), 1);
        assert_eq!(p.sections[0].text, long.trim());
        assert!(!p.truncated);
        assert!(preview(&conn, "/not/indexed.txt").is_err());
    }

    #[test]
    fn one_hit_per_file_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open(&dir.path().join("index.db")).unwrap();
        conn.execute("INSERT INTO folders (id, path) VALUES (1, '/d')", []).unwrap();
        let mut semantic = Vec::new();
        for f in 0..5 {
            let file = crate::extract::Candidate { path: format!("/d/{f}.txt").into(), mtime: 0, size: 1, kind: "text", cloud: false };
            let rows = (0..3)
                .map(|p| crate::index::Row { page: Some(p), text: format!("file {f} page {p}"), vec: FakeEmbedder::vector("x") })
                .collect();
            for (id, _) in write_file(&conn, 1, &file, rows, None, false).unwrap().1 {
                semantic.push((id, 0.9));
            }
        }
        let hits = rank(&conn, &semantic, "", "all", None, 3).unwrap();
        assert_eq!(names(&hits), ["0.txt", "1.txt", "2.txt"]);
        assert_eq!(hits[0].page, Some(0));
    }
}
