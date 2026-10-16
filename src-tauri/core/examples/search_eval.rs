//! Measures search quality and speed on a labelled test set, by default the Traditional
//! Chinese one in `eval/zh-tw`. It indexes the corpus with the app's own extraction,
//! chunking and storage, then runs every query through the same ranking the app uses,
//! three ways: semantic only, keyword only, and both fused (what the app shows).
//!
//! ```text
//! cargo run --release -p nlsearch-core --example search_eval -- --server http://127.0.0.1:8080
//! cargo run --release -p nlsearch-core --example search_eval -- --embedder bigram
//! ```
//!
//! `--server` uses a running llama-server with EmbeddingGemma (`scripts/eval-zh.ps1`
//! starts one from the app's runtime folder). Without a model, `--embedder bigram` uses a
//! character-bigram stand-in that only shows what plain character overlap achieves, and
//! `--embedder none` measures the keyword side alone. `--out results.json` keeps the
//! numbers for comparison, `--scale 100000` times the vector scan on that many vectors.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use nlsearch_core::db::{self, VecCache};
use nlsearch_core::embed::{normalize, Embedder, HttpEmbedder, Input, DIM};
use nlsearch_core::extract::{extract, scan, Exclusions};
use nlsearch_core::index::{embed_items, write_file};
use nlsearch_core::search::{candidates, query_input, rank};
use nlsearch_core::text::is_cjk;
use rusqlite::params;
use serde_json::json;

const LIMIT: usize = 10;
const MODES: [&str; 3] = ["hybrid", "semantic", "keyword"];

struct Query {
    category: String,
    text: String,
    relevant: HashSet<String>,
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let data = flag("--data")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../eval/zh-tw"));
    let embedder: Box<dyn Embedder> = match (flag("--server"), flag("--embedder").as_deref()) {
        (Some(url), _) => Box::new(HttpEmbedder::new(&url)),
        (None, Some("bigram") | None) => Box::new(BigramEmbedder),
        (None, Some("none")) => Box::new(NoEmbedder),
        (None, Some(other)) => bail!("unknown embedder {other}: use bigram or none, or --server URL"),
    };
    let embedder_name = match (flag("--server"), flag("--embedder")) {
        (Some(url), _) => format!("llama-server {url}"),
        (None, e) => e.unwrap_or_else(|| "bigram".into()),
    };
    let semantic_on = embedder_name != "none";

    let queries = load_queries(&data.join("queries.tsv"))?;
    let tmp = tempfile::tempdir()?;
    let db_path = tmp.path().join("index.db");
    let conn = db::open(&db_path)?;
    let corpus = data.join("corpus").canonicalize().context("corpus folder missing")?;
    conn.execute("INSERT INTO folders (id, path) VALUES (1, ?1)", params![corpus.to_string_lossy()])?;

    // Indexing, timed per step the way the app runs it (minus threads).
    let mut cache = VecCache::default();
    let (mut t_extract, mut t_embed, mut t_write) = (Duration::ZERO, Duration::ZERO, Duration::ZERO);
    let (mut files, mut chunks, mut chars, mut max_chunk) = (0usize, 0usize, 0usize, 0usize);
    let index_start = Instant::now();
    for file in scan(&corpus, &Exclusions::default()) {
        let t = Instant::now();
        let (items, thumb) = extract(&file, None).unwrap_or_default();
        t_extract += t.elapsed();
        for item in &items {
            let n = item.text.chars().count();
            chars += n;
            max_chunk = max_chunk.max(n);
        }
        chunks += items.len();
        let t = Instant::now();
        let rows = if semantic_on {
            embed_items(embedder.as_ref(), items, || true).context("embedding stopped")?
        } else {
            items.into_iter().map(|i| nlsearch_core::index::Row { page: i.page, text: i.text, vec: vec![0.0; DIM] }).collect()
        };
        t_embed += t.elapsed();
        let t = Instant::now();
        let (file_id, added) = write_file(&conn, 1, &file, rows, thumb.as_deref(), false)?;
        for (id, vec) in added {
            cache.push(id, file_id, db::kind_code(file.kind), &vec);
        }
        t_write += t.elapsed();
        files += 1;
    }
    let t_index = index_start.elapsed();
    if semantic_on {
        // Warm-up, so the first query does not pay for the model's first request.
        embedder.embed(&[query_input("暖身")])?;
    }

    // Queries.
    let mut per_mode: BTreeMap<&str, Vec<(String, Option<usize>)>> = BTreeMap::new();
    let (mut t_qembed, mut t_scan, mut t_rank) = (Vec::new(), Vec::new(), Vec::new());
    let mut misses = Vec::new();
    for q in &queries {
        let t = Instant::now();
        let qvec = if semantic_on { embedder.embed(&[query_input(&q.text)])?.remove(0) } else { vec![0.0; DIM] };
        t_qembed.push(t.elapsed());
        let t = Instant::now();
        let semantic = if semantic_on { cache.top_k(&qvec, candidates(LIMIT), |_, _| true) } else { Vec::new() };
        t_scan.push(t.elapsed());

        for mode in MODES {
            let t = Instant::now();
            let hits = match mode {
                "hybrid" => rank(&conn, &semantic, &q.text, "all", None, LIMIT)?,
                "semantic" => rank(&conn, &semantic, "", "all", None, LIMIT)?,
                _ => rank(&conn, &[], &q.text, "all", None, LIMIT)?,
            };
            if mode == "hybrid" {
                t_rank.push(t.elapsed());
            }
            let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
            let first = names.iter().position(|n| q.relevant.contains(*n));
            if mode == "hybrid" && first.map_or(true, |r| r >= 5) {
                misses.push(json!({
                    "category": q.category, "query": q.text,
                    "expected": q.relevant.iter().collect::<Vec<_>>(),
                    "rank": first.map(|r| r + 1), "top3": names.iter().take(3).collect::<Vec<_>>(),
                }));
            }
            per_mode.entry(mode).or_default().push((q.category.clone(), first));
        }
    }

    // Report.
    let mut categories: Vec<String> = Vec::new();
    for q in &queries {
        if !categories.contains(&q.category) {
            categories.push(q.category.clone());
        }
    }
    println!("# Search evaluation: {} queries, {} files, embedder: {embedder_name}\n", queries.len(), files);
    println!("| mode | Hit@1 | Hit@5 | Hit@10 | MRR@10 |\n|---|---|---|---|---|");
    let mut summary = serde_json::Map::new();
    for mode in MODES {
        let all = &per_mode[mode];
        let m = metrics(all.iter().map(|(_, r)| *r));
        println!("| {mode} | {:.3} | {:.3} | {:.3} | {:.3} |", m[0], m[1], m[2], m[3]);
        let mut by_cat = serde_json::Map::new();
        for c in &categories {
            let m = metrics(all.iter().filter(|(cat, _)| cat == c).map(|(_, r)| *r));
            by_cat.insert(c.clone(), json!({ "hit1": m[0], "hit5": m[1], "hit10": m[2], "mrr": m[3] }));
        }
        summary.insert(mode.into(), json!({ "hit1": m[0], "hit5": m[1], "hit10": m[2], "mrr": m[3], "by_category": by_cat }));
    }

    println!("\n## Hit@5 / MRR@10 by category\n");
    println!("| category | n | {} |\n|---|---|{}", MODES.join(" | "), "---|".repeat(MODES.len()));
    for c in &categories {
        let n = queries.iter().filter(|q| &q.category == c).count();
        let cells: Vec<String> = MODES
            .iter()
            .map(|mode| {
                let s = &summary[*mode]["by_category"][c];
                format!("{:.2} / {:.2}", s["hit5"].as_f64().unwrap(), s["mrr"].as_f64().unwrap())
            })
            .collect();
        println!("| {c} | {n} | {} |", cells.join(" | "));
    }

    let db_bytes = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
    let speed = json!({
        "index_seconds": t_index.as_secs_f64(),
        "extract_seconds": t_extract.as_secs_f64(),
        "embed_seconds": t_embed.as_secs_f64(),
        "write_seconds": t_write.as_secs_f64(),
        "files": files, "chunks": chunks, "chars": chars, "longest_chunk_chars": max_chunk,
        "chunks_per_second": chunks as f64 / t_embed.as_secs_f64().max(1e-9),
        "query_embed_ms": percentiles(&t_qembed),
        "vector_scan_ms": percentiles(&t_scan),
        "rank_ms": percentiles(&t_rank),
        "vector_cache_bytes": cache_bytes(cache.ids.len()),
        "db_bytes": db_bytes,
        "peak_rss_bytes": peak_rss(),
    });
    println!("\n## Speed and size\n");
    println!("- indexing: {files} files, {chunks} passages, {chars} characters in {:.2} s (extract {:.2} s, embed {:.2} s, write {:.2} s); {:.1} passages/s while embedding",
        t_index.as_secs_f64(), t_extract.as_secs_f64(), t_embed.as_secs_f64(), t_write.as_secs_f64(), speed["chunks_per_second"].as_f64().unwrap());
    println!("- longest passage: {max_chunk} characters");
    for (label, key) in [("query embedding", "query_embed_ms"), ("vector scan", "vector_scan_ms"), ("fusion and lookup", "rank_ms")] {
        let p = &speed[key];
        println!("- {label}: p50 {:.2} ms, p95 {:.2} ms", p["p50"].as_f64().unwrap(), p["p95"].as_f64().unwrap());
    }
    println!("- vector cache {:.1} KB, database {:.1} KB, peak RSS {}", cache_bytes(cache.ids.len()) as f64 / 1024.0, db_bytes as f64 / 1024.0,
        peak_rss().map_or("n/a".into(), |b| format!("{:.1} MB", b as f64 / 1048576.0)));

    let mut scale = serde_json::Value::Null;
    if let Some(n) = flag("--scale").and_then(|s| s.parse::<usize>().ok()) {
        let (scan_ms, bytes) = scale_scan(n);
        println!("- vector scan over {n} synthetic passages: p50 {:.2} ms, p95 {:.2} ms, {:.0} MB of vectors",
            scan_ms["p50"].as_f64().unwrap(), scan_ms["p95"].as_f64().unwrap(), bytes as f64 / 1048576.0);
        scale = json!({ "passages": n, "scan_ms": scan_ms, "vector_bytes": bytes });
    }

    println!("\n## Fused results missing the top 5 ({})\n", misses.len());
    for m in &misses {
        println!("- [{}] {} → expected {}, got {}", m["category"].as_str().unwrap(), m["query"].as_str().unwrap(),
            m["expected"], m["top3"]);
    }

    if let Some(out) = flag("--out") {
        let report = json!({
            "embedder": embedder_name, "queries": queries.len(), "metrics": summary,
            "speed": speed, "scale": scale, "misses": misses,
        });
        std::fs::write(&out, serde_json::to_string_pretty(&report)?)?;
        println!("\nWrote {out}");
    }
    Ok(())
}

fn load_queries(path: &Path) -> Result<Vec<Query>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')) {
        let cols: Vec<&str> = line.split('\t').collect();
        let [category, query, relevant] = cols[..] else { bail!("bad line in queries.tsv: {line}") };
        out.push(Query {
            category: category.into(),
            text: query.into(),
            relevant: relevant.split(';').map(|s| s.trim().to_string()).collect(),
        });
    }
    Ok(out)
}

/// Hit@1, Hit@5, Hit@10 and MRR@10 from the 0-based rank of the first relevant file.
fn metrics(ranks: impl Iterator<Item = Option<usize>>) -> [f64; 4] {
    let ranks: Vec<Option<usize>> = ranks.collect();
    let n = ranks.len().max(1) as f64;
    let hit = |k: usize| ranks.iter().filter(|r| r.is_some_and(|r| r < k)).count() as f64 / n;
    let mrr = ranks.iter().map(|r| r.filter(|r| *r < LIMIT).map_or(0.0, |r| 1.0 / (r + 1) as f64)).sum::<f64>() / n;
    [hit(1), hit(5), hit(10), mrr]
}

fn percentiles(samples: &[Duration]) -> serde_json::Value {
    let mut ms: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    let at = |p: f64| ms.get(((ms.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(0.0);
    json!({ "p50": at(0.5), "p95": at(0.95), "max": at(1.0) })
}

/// Times `VecCache::top_k` over `n` random unit vectors, as a search over a large index would.
fn scale_scan(n: usize) -> (serde_json::Value, usize) {
    let mut cache = VecCache::default();
    let mut seed = 0x2545F4914F6CDD1Du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 40) as f32 / (1u64 << 24) as f32 - 0.5
    };
    let mut v = vec![0f32; DIM];
    for i in 0..n {
        v.iter_mut().for_each(|x| *x = next());
        normalize(&mut v);
        cache.push(i as i64, (i / 4) as i64, 0, &v);
    }
    let mut times = Vec::new();
    for _ in 0..20 {
        v.iter_mut().for_each(|x| *x = next());
        normalize(&mut v);
        let t = Instant::now();
        std::hint::black_box(cache.top_k(&v, candidates(LIMIT), |_, _| true));
        times.push(t.elapsed());
    }
    (percentiles(&times), cache_bytes(n))
}

/// Memory the vector cache uses for `n` passages: one byte per dimension plus a scale.
fn cache_bytes(n: usize) -> usize {
    n * (DIM + 4)
}

#[cfg(target_os = "linux")]
fn peak_rss() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse::<u64>().ok().map(|kb| kb * 1024)
}

#[cfg(not(target_os = "linux"))]
fn peak_rss() -> Option<u64> {
    None
}

/// A stand-in for the model: hashed character unigrams and bigrams of CJK text, and
/// lower-cased Latin words. It knows no meaning, only shared characters, so it marks the
/// floor a real model has to clear.
struct BigramEmbedder;

impl Embedder for BigramEmbedder {
    fn embed(&self, inputs: &[Input]) -> Result<Vec<Vec<f32>>> {
        Ok(inputs
            .iter()
            .map(|input| {
                let text = match input {
                    Input::Text(t) => t.as_str(),
                    Input::Image(_) => "",
                };
                let text = text.strip_prefix("task: search result | query: ").unwrap_or(text);
                let text = text.strip_prefix("title: ").unwrap_or(text).replacen(" | text: ", " ", 1);
                let mut v = vec![0f32; DIM];
                let mut bump = |key: &str, w: f32| {
                    let h = key.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
                    v[(h % DIM as u64) as usize] += w;
                };
                let chars: Vec<char> = text.chars().collect();
                let mut word = String::new();
                for (i, c) in chars.iter().enumerate() {
                    if is_cjk(*c) {
                        bump(&c.to_string(), 0.5);
                        if chars.get(i + 1).is_some_and(|n| is_cjk(*n)) {
                            bump(&format!("{c}{}", chars[i + 1]), 1.0);
                        }
                    }
                    if c.is_alphanumeric() && !is_cjk(*c) {
                        word.extend(c.to_lowercase());
                    } else if !word.is_empty() {
                        bump(&std::mem::take(&mut word), 1.0);
                    }
                }
                if !word.is_empty() {
                    bump(&word, 1.0);
                }
                normalize(&mut v);
                v
            })
            .collect())
    }
}

/// Keyword search only.
struct NoEmbedder;

impl Embedder for NoEmbedder {
    fn embed(&self, inputs: &[Input]) -> Result<Vec<Vec<f32>>> {
        Ok(inputs.iter().map(|_| vec![0.0; DIM]).collect())
    }
}
