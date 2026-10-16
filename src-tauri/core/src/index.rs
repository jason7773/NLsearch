//! The steps of an indexing pass that touch the database or the model. The app runs
//! them on its threads and reports progress; this module only does the work.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{params, Connection};

use crate::db;
use crate::embed::{Embedder, Input};
use crate::extract::{Candidate, Item};

/// Inputs per embedding request. Kept small so a search never waits long behind one.
pub const EMBED_BATCH: usize = 8;

/// One embedded passage, ready to store.
pub struct Row {
    pub page: Option<i64>,
    pub text: String,
    pub vec: Vec<f32>,
}

/// Drops files whose folder was removed while they were being written, then folds
/// nested folders into their outermost indexed ancestor.
pub fn tidy(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DELETE FROM chunks WHERE file_id IN
             (SELECT id FROM files WHERE folder_id NOT IN (SELECT id FROM folders));
         DELETE FROM files WHERE folder_id NOT IN (SELECT id FROM folders);
         DELETE FROM thumbs WHERE file_id NOT IN (SELECT id FROM files);",
    )?;
    db::merge_nested_folders(conn)
}

/// Compares what is on disk in a folder with what the index holds. Files that changed
/// or disappeared are removed from the index; the new and changed ones are returned to
/// be indexed, and recorded as the folder's pending count.
pub fn plan_folder(conn: &Connection, folder_id: i64, found: Vec<Candidate>) -> Result<Vec<Candidate>> {
    let mut known: HashMap<String, (i64, i64, u64)> = conn
        .prepare("SELECT path, id, mtime, size FROM files WHERE folder_id = ?1")?
        .query_map(params![folder_id], |r| {
            Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get::<_, i64>(3)? as u64)))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let mut todo = Vec::new();
    for file in found {
        let key = file.path.to_string_lossy().into_owned();
        match known.remove(&key) {
            Some((_, mtime, size)) if mtime == file.mtime && size == file.size => {}
            Some((id, ..)) => {
                db::delete_file(conn, id)?;
                todo.push(file);
            }
            None => todo.push(file),
        }
    }
    // Whatever is left no longer exists on disk.
    for (id, ..) in known.into_values() {
        db::delete_file(conn, id)?;
    }
    conn.execute(
        "UPDATE folders SET pending = ?1 WHERE id = ?2",
        params![todo.len() as i64, folder_id],
    )?;
    Ok(todo)
}

/// Embeds a file's items in batches. `proceed` runs before each batch: it may wait
/// (the app lets searches go first) and returns false to stop, which gives None so
/// that a stopped file is not written at all and the next pass redoes it whole.
pub fn embed_items(
    embedder: &dyn Embedder,
    items: Vec<Item>,
    mut proceed: impl FnMut() -> bool,
) -> Option<Vec<Row>> {
    let mut rows = Vec::new();
    let mut items = items.into_iter().peekable();
    while items.peek().is_some() {
        if !proceed() {
            return None;
        }
        let batch: Vec<Item> = items.by_ref().take(EMBED_BATCH).collect();
        let (meta, inputs): (Vec<_>, Vec<Input>) =
            batch.into_iter().map(|i| ((i.page, i.text), i.input)).unzip();
        match embedder.embed(&inputs) {
            Ok(vecs) => rows.extend(meta.into_iter().zip(vecs).map(|((page, text), vec)| Row { page, text, vec })),
            // One bad input fails the whole request: retry singly and drop the culprit.
            Err(_) => {
                for ((page, text), input) in meta.into_iter().zip(inputs) {
                    if let Ok(mut v) = embedder.embed(std::slice::from_ref(&input)) {
                        rows.push(Row { page, text, vec: v.remove(0) });
                    }
                }
            }
        }
    }
    Some(rows)
}

/// Writes one file with its passages and preview in one transaction, and returns the
/// new chunk ids with their vectors. Files with nothing indexable are still recorded
/// so they are not retried every pass.
pub fn write_file(
    conn: &Connection,
    folder_id: i64,
    file: &Candidate,
    rows: Vec<Row>,
    thumb: Option<&[u8]>,
) -> Result<Vec<(i64, Vec<f32>)>> {
    let mut added = Vec::new();
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT OR REPLACE INTO files (folder_id, path, mtime, size, kind) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![folder_id, file.path.to_string_lossy(), file.mtime, file.size as i64, file.kind],
    )?;
    let file_id = tx.last_insert_rowid();
    for row in rows {
        tx.execute(
            "INSERT INTO chunks (file_id, page, text, emb) VALUES (?1, ?2, ?3, ?4)",
            params![file_id, row.page, row.text, db::vec_to_blob(&row.vec)],
        )?;
        added.push((tx.last_insert_rowid(), row.vec));
    }
    if let Some(jpeg) = thumb {
        tx.execute(
            "INSERT OR REPLACE INTO thumbs (file_id, jpeg) VALUES (?1, ?2)",
            params![file_id, jpeg],
        )?;
    }
    tx.execute(
        "UPDATE folders SET pending = MAX(pending - 1, 0) WHERE id = ?1",
        params![folder_id],
    )?;
    tx.commit()?;
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeEmbedder;

    fn item(text: &str) -> Item {
        Item { page: None, text: text.into(), input: Input::Text(text.into()) }
    }

    fn candidate(path: &str, mtime: i64) -> Candidate {
        Candidate { path: path.into(), mtime, size: 10, kind: "text" }
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn embeds_in_batches_and_drops_only_the_bad_input() {
        let embedder = FakeEmbedder { fail_on: Some("poison".into()) };
        let items: Vec<Item> = (0..10).map(|i| item(&format!("passage {i}"))).chain([item("poison")]).collect();
        let mut batches = 0;
        let rows = embed_items(&embedder, items, || {
            batches += 1;
            true
        })
        .unwrap();
        assert_eq!(batches, 2);
        assert_eq!(rows.len(), 10);
        assert!(rows.iter().all(|r| r.text != "poison"));
    }

    #[test]
    fn a_stop_writes_nothing() {
        let rows = embed_items(&FakeEmbedder::new(), vec![item("a")], || false);
        assert!(rows.is_none());
    }

    #[test]
    fn plan_finds_new_changed_and_deleted_files() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open(&dir.path().join("index.db")).unwrap();
        conn.execute("INSERT INTO folders (id, path) VALUES (1, '/docs')", []).unwrap();
        let vec = FakeEmbedder::vector("x");
        for (path, mtime) in [("/docs/same.txt", 1), ("/docs/changed.txt", 1), ("/docs/gone.txt", 1)] {
            let row = Row { page: None, text: "x".into(), vec: vec.clone() };
            write_file(&conn, 1, &candidate(path, mtime), vec![row], None).unwrap();
        }

        let found = vec![candidate("/docs/same.txt", 1), candidate("/docs/changed.txt", 2), candidate("/docs/new.txt", 1)];
        let todo = plan_folder(&conn, 1, found).unwrap();
        let mut paths: Vec<_> = todo.iter().map(|c| c.path.to_string_lossy().into_owned()).collect();
        paths.sort();
        assert_eq!(paths, ["/docs/changed.txt", "/docs/new.txt"]);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM files"), 1);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM chunks"), 1);
        assert_eq!(count(&conn, "SELECT pending FROM folders WHERE id = 1"), 2);
    }

    #[test]
    fn tidy_merges_nested_folders_and_drops_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open(&dir.path().join("index.db")).unwrap();
        conn.execute_batch(
            "INSERT INTO folders (id, path) VALUES (1, '/docs'), (2, '/docs/sub');
             INSERT INTO files (folder_id, path, mtime, size, kind) VALUES (2, '/docs/sub/a.txt', 1, 1, 'text');
             INSERT INTO files (folder_id, path, mtime, size, kind) VALUES (9, '/removed/b.txt', 1, 1, 'text');",
        )
        .unwrap();
        tidy(&conn).unwrap();
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM folders"), 1);
        assert_eq!(count(&conn, "SELECT folder_id FROM files"), 1);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM files"), 1);
    }
}
