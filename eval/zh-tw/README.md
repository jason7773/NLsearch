# Traditional Chinese search test set

A small, hand-labelled set for measuring how well NLsearch finds Traditional Chinese
files, and for checking that a change to chunking, the keyword index or ranking helps.

- `corpus/`: 42 short files of the kind people keep (leases, bills, notes, travel plans,
  a health report, payslips), with near neighbours on purpose (two leases, two
  electricity bills, two sets of meeting notes) and a few long ones that split into
  several passages. Two are in English and one in Simplified Chinese.
- `queries.tsv`: 91 queries, each with the file(s) that answer it, in these categories:

| category | what it tests | example |
|---|---|---|
| keyword | the words appear as typed | 凱米颱風 |
| natural | a spoken-style question, not found word for word | 大安區那間房子押金要付多少 |
| synonym | the file uses another word | 摩托車保養 → 機車 |
| simplified | Simplified query, Traditional file | 体检报告 胆固醇 |
| to_simplified | Traditional query, Simplified file | 上海出差報銷 |
| crosslingual | Chinese ⇄ English | 波士頓租屋押金, beef noodle soup recipe |
| variant | variant characters | 家裡 → 家裏, 臺北 → 台北 |
| long | the answer sits in the middle of a long file | 居家服務每小時補助多少錢 |
| filename | the words appear only in the file name | 戴森吸塵器保固 |
| number | numbers, dates and codes | ABC-1234 |

## Running it

With the real model, on Windows, from the repository root (starts llama-server from the
app's runtime folder, then stops it):

```
powershell -ExecutionPolicy Bypass -File scripts\eval-zh.ps1
```

Against a llama-server that is already running, or without a model:

```
cd src-tauri
cargo run --release -p nlsearch-core --example search_eval -- --server http://127.0.0.1:8080
cargo run --release -p nlsearch-core --example search_eval -- --embedder bigram
cargo run --release -p nlsearch-core --example search_eval -- --embedder none
```

`bigram` is a stand-in that only counts shared characters, to show the floor a real
model must clear; `none` measures the keyword side alone. Add `--out file.json` to keep
the numbers and `--scale 100000` to time the vector scan over a large index.

Each query is run three ways through the app's own `search::rank`: semantic only,
keyword only, and fused (what the app shows). Reported per mode and per category:
Hit@k (the share of queries with a right file in the top k) and MRR@10 (the mean of
1 / rank of the first right file). Speed covers indexing, query embedding, the vector
scan and fusion.

## Results so far

Measured in a Linux container without the model (Hit@5 / MRR@10, fused):

| | keyword side alone | fused, bigram stand-in |
|---|---|---|
| before: every query word as an exact phrase | 0.18 / 0.18 | 0.80 / 0.74 |
| after: character pairs ranked by bm25 | 0.79 / 0.75 | 0.84 / 0.79 |

Vector scan over 100,000 passages (4 cores): 105 ms and 293 MB at first; about 45 ms
after selecting the top results instead of sorting all of them and vectorising the dot
product; about 9 ms and 74 MB once the cache holds 8-bit vectors and large caches are
scanned on every core. The 8-bit vectors left the fused scores unchanged.

Numbers with EmbeddingGemma 2 still need a run on Windows with `eval-zh.ps1`.

## Adding queries

One query per line: `category<TAB>query<TAB>file;file`. Keep the corpus free of
personal data; everything here is made up.
