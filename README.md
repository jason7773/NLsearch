# FileDig

Local semantic file search for Windows. Type what you are looking for in plain language and
FileDig finds matching text files, PDFs and images by meaning, not just by file name.

Everything runs on your machine: the model is [EmbeddingGemma 2](https://huggingface.co/google/embeddinggemma-2)
running through [llama.cpp](https://github.com/ggml-org/llama.cpp), on an NVIDIA GPU (CUDA) when one is
present and on the CPU otherwise. Inspired by [DigUp](https://github.com/ARahim3/DigUp) for macOS.

## Features

- **Semantic search** across text and images in one index: "how do I fix my wifi", "a red circle",
  "scanned bill with a total amount".
- **Exact-match search** for codes, names and numbers, merged with the semantic results.
- **PDF results point to the page** that matched.
- **Filters** for All, Documents or Images.
- **Incremental indexing**: only new or changed files are processed on a rescan.
- **Stop and resume**: indexing can be stopped at any time. A banner warns while a folder is not
  fully indexed, and the Folders panel shows how many files are still waiting.
- **Tray app** with a global hotkey. Closing the window keeps it running in the tray.
- **Frees VRAM when idle**: the model unloads after 5 minutes without use and reloads on the next search.
- **Read-only**: it never changes, moves or uploads your files.

### Supported files

| Kind | Extensions |
|---|---|
| Text | txt, md, markdown, rst, log, csv, html, htm, docx |
| PDF | pdf (text layer, per page) |
| Images | jpg, jpeg, png, webp, bmp, gif |

## Requirements

- Windows 10/11 x64 (WebView2, included with Windows 11)
- For GPU: an NVIDIA card with a recent driver. The model uses about 3.6 GB of VRAM while loaded.
- To build: [Rust](https://rustup.rs), [Node.js](https://nodejs.org) and [pnpm](https://pnpm.io)

## Setup

### 1. Install dependencies

```
pnpm install
```

### 2. Create the `runtime` folder

`runtime/` holds the llama.cpp binaries and the model. It is not in git (about 1.5 GB).

```
runtime/
  llama-cuda/    llama-server.exe + DLLs (CUDA build, plus the cudart DLLs)
  llama-cpu/     llama-server.exe + DLLs (CPU build)
  models/
    embeddinggemma-2-Q8_0.gguf
    mmproj-embeddinggemma-2-Q8_0.gguf
```

**llama.cpp** (build b11552 or later, which includes EmbeddingGemma 2 support), from the
[releases page](https://github.com/ggml-org/llama.cpp/releases):

- `llama-<build>-bin-win-cuda-12.4-x64.zip` and `cudart-llama-bin-win-cuda-12.4-x64.zip`,
  both extracted into `runtime/llama-cuda/`
- `llama-<build>-bin-win-cpu-x64.zip`, extracted into `runtime/llama-cpu/`

Only one of the two folders is required. With both present, CUDA is used when an NVIDIA GPU is
detected and CPU is the fallback.

**Model files**, from [ggml-org/embeddinggemma-2-GGUF](https://huggingface.co/ggml-org/embeddinggemma-2-GGUF):

```
curl -L -C - -o runtime/models/embeddinggemma-2-Q8_0.gguf https://huggingface.co/ggml-org/embeddinggemma-2-GGUF/resolve/main/embeddinggemma-2-Q8_0.gguf
curl -L -C - -o runtime/models/mmproj-embeddinggemma-2-Q8_0.gguf https://huggingface.co/ggml-org/embeddinggemma-2-GGUF/resolve/main/mmproj-embeddinggemma-2-Q8_0.gguf
```

`-C -` resumes a partial download if the connection drops; rerun the command until it completes.

To keep the runtime somewhere else, set the `FILEDIG_RUNTIME` environment variable to its path.

Other optional environment variables:

- `FILEDIG_DATA`: folder for the index, instead of the per-user app data folder.
- `FILEDIG_INDEX_THREADS`: how many files are read at once while indexing (default: half the logical cores, at most 8).

### 3. Run

```
pnpm tauri dev
```

To build a release executable:

```
pnpm tauri build
```

The built app looks for `runtime/` next to the executable or in any parent folder.

## Portable build

```
pwsh scripts\package-portable.ps1          # GPU (CUDA) + CPU
pwsh scripts\package-portable.ps1 -CpuOnly # CPU only, much smaller
pwsh scripts\package-portable.ps1 -Zip     # also write a .zip
```

This writes `dist-portable\FileDig\`, a folder that runs from anywhere without installation:

```
FileDig\
  FileDig.exe
  runtime\     llama.cpp server and the model
  data\        index, logs and browser profile (created empty)
```

The app is portable whenever a `data` folder sits beside the executable: everything it writes goes
there instead of `%APPDATA%`. It needs the Microsoft Edge WebView2 Runtime, which is part of
Windows 11 and of up-to-date Windows 10. Indexed folders are stored by full path, so on another PC
they have to be added again.

## Usage

1. Click **Folders**, then **Add folder** and pick a folder, or drag a folder into the window. Indexing starts immediately and the
   status bar shows progress.
2. Type a query. Results update as you type.
3. Click **Stop** in the banner to interrupt indexing; files already indexed stay searchable.
   **Resume indexing** continues where it left off.
4. Use **Rescan** to pick up files added, changed or deleted since the last pass. A rescan also
   runs every time the app starts.

| Key | Action |
|---|---|
| Ctrl+Shift+Space | Show or hide the window from anywhere |
| Up / Down | Move the selection |
| Enter | Open the selected file |
| Ctrl+Enter or Ctrl+click | Show the file in Explorer |
| Esc | Hide the window |

To quit, right-click the tray icon and choose **Quit**.

Each result shows a similarity score between 0 and 1. Results marked **exact match** contain every
word of the query.

## How it works

- **Engine** (`src-tauri/src/engine.rs`): starts `llama-server` as a hidden helper process on a free
  local port and requests embeddings over HTTP. Text and images are embedded into the same
  768-dimensional space, so a text query can match either. The helper is tied to the app with a
  Windows job object, so it never outlives the app.
- **Indexer** (`src-tauri/src/indexer.rs`): walks the folders, splits text into overlapping passages
  of about 1,200 characters, extracts PDF text page by page, and downsizes images to 896 px before
  embedding. Hidden folders and folders such as `node_modules` are skipped.
- **Storage** (`src-tauri/src/db.rs`): one SQLite file holds the files, passages, vectors and an FTS5
  keyword index. Vectors are searched by brute force in memory.
- **Search** (`src-tauri/src/lib.rs`): the semantic and keyword rankings are merged with reciprocal
  rank fusion, and the best passage per file is shown.
- **UI** (`src/`): plain TypeScript and CSS, built with Vite.

The index is stored at `%APPDATA%\com.rupesh.filedig\index.db`, with the llama.cpp logs beside it.
Deleting that file resets the index.

## Performance

Measured on an RTX 3060 12 GB and a Core i7-12700:

| | CUDA | CPU |
|---|---|---|
| Query | 50–80 ms | not measured end to end |
| Embedding one image | not measured separately | about 1.1 s |
| Embedding one text passage | not measured separately | about 0.26 s |
| Model load | a few seconds | not measured |

Indexing a test folder of 8 files (9 passages and images) took about 7 s on CUDA, including model load.

## Limitations

- No live file watching yet: changes are picked up on startup or with **Rescan**.
- Scanned PDFs without a text layer are skipped, and there is no OCR for screenshots.
- Audio, video and source code are not indexed.
- In the **All** view, image and text scores sit on slightly different scales, so an unrelated image
  can outrank a weak text match. The Documents and Images filters avoid this.
- No installer: the `runtime` folder has to be placed by hand.
- Size limits: text files over 5 MB, images over 50 MB and PDFs over 100 MB are skipped.

## License

FileDig is released under the [MIT License](LICENSE).

The components it runs are licensed separately: EmbeddingGemma 2 under Apache 2.0 and llama.cpp under MIT.
