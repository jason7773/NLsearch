/**
 * Interface text in English and Traditional Chinese. The language follows Windows unless
 * one is chosen in Settings. Keys ending in _one and _other are singular and plural
 * forms, picked by `tn`; {name} in a text is filled from `vars`.
 */

export type Language = "en" | "zh-TW";
export type LanguageChoice = "system" | Language;

const en = {
  // Header and toolbar
  "search.placeholder": "Search files by what they contain",
  "search.clear": "Clear",
  "tab.all": "All",
  "tab.docs": "Documents",
  "tab.images": "Images",
  folders: "Folders",
  "folders.title": "Choose which folders are searched",
  "filters.title": "Filter by date, folder or file type",
  "settings.title": "Settings",
  "about.title": "About NLsearch",
  "preview.toggle": "Show or hide the preview",
  "sort.relevance": "Best match",
  "sort.newest": "Newest first",
  "sort.oldest": "Oldest first",
  "sort.title": "Order of the results",
  "meta.results_one": "{n} result · {ms} ms",
  "meta.results_other": "{n} results · {ms} ms",
  "similar.to": "Like {name}",
  "similar.clear": "Back to search",

  // Filters
  "filter.anytime": "Any time",
  "filter.day": "Past day",
  "filter.week": "Past week",
  "filter.month": "Past month",
  "filter.year": "Past year",
  "filter.allFolders": "All folders",
  "filter.allTypes": "All file types",
  "filter.clear": "Clear",
  "type.pdf": "PDF",
  "type.word": "Word",
  "type.excel": "Excel and CSV",
  "type.powerpoint": "PowerPoint",
  "type.text": "Text and web pages",
  "type.code": "Code",
  "type.pictures": "Pictures",

  // File kinds, as shown under a result
  "kind.pdf": "PDF",
  "kind.word": "Word document",
  "kind.csv": "CSV",
  "kind.web": "Web page",
  "kind.markdown": "Markdown",
  "kind.text": "Text",
  "kind.log": "Log",
  "kind.excel": "Excel workbook",
  "kind.powerpoint": "PowerPoint",
  "kind.code": "Code",
  "kind.image": "{ext} image",
  page: "Page {n}",
  slide: "Slide {n}",

  // Results
  "match.exact": "Exact match",
  "match.strong": "Strong match",
  "match.good": "Good match",
  "match.related": "Related",
  "match.similarity": "Similarity {score}",
  "action.open": "Open",
  "action.reveal": "Show in folder",
  "action.similar": "Find similar",
  "action.copyPath": "Copy path",
  "action.copyText": "Copy text",
  "copied.path": "Path copied.",
  "copied.text": "Text copied.",
  "copy.failed": "Could not copy: {error}",
  "open.failed": "Could not open {name}: {error}",
  more: "Show more results",
  "noResults.title": "No results",
  "noResults.detail": "Try different words, or another filter.",
  "noSimilar.detail": "Nothing else in the index is much like this file.",
  "searchFailed.title": "Search failed",

  // Preview
  "preview.empty": "Select a result to see it here.",
  "preview.loading": "Loading…",
  "preview.none": "No text was found in this file.",
  "preview.truncated": "The preview shows the start of a long file. Open it to read the rest.",
  "preview.failed": "No preview: {error}",

  // Start screen
  "idle.title": "Search your files",
  "idle.detail": "Find documents and pictures by what they contain, or by an exact name or number.",
  "idle.recentSearches": "Recent searches",
  "idle.recentFiles": "Recently opened",
  "idle.forget": "Remove from history",
  "idle.try": "Try",
  "suggest.1": "invoice with a total amount",
  "suggest.2": "photo of a sunset",
  "suggest.3": "wifi password",
  "suggest.4": "rental agreement",
  "addFolder.title": "Add a folder to search",
  "addFolder.detail":
    "NLsearch reads the documents and pictures in the folders you choose, so you can find them by what they contain.",
  "addFolder.drag": "You can also drag a folder here.",
  "setup.title": "One download to get started",
  "setup.detailGpu":
    "NLsearch needs its search model. An NVIDIA graphics card was found, so GPU support is included to make indexing several times faster. Everything runs on this computer.",
  "setup.detail": "NLsearch needs its search model. It is downloaded once and then runs entirely on this computer.",
  "setup.both": "Download model and GPU support ({size})",
  "setup.model": "Download model ({size})",
  "setup.modelOnly": "Model only ({size})",

  // Status bar
  "status.indexed_one": "{n} file indexed",
  "status.indexed_other": "{n} files indexed",
  "status.progress": "{phase} {done} of {total}",
  "status.error": "Indexing error: {error}",
  "phase.Scanning": "Scanning",
  "phase.Loading model": "Loading model",
  "phase.Indexing": "Indexing",
  "backend.gpu": "GPU",
  "backend.cpuGpu": "CPU · GPU available",
  "backend.loaded": "Model loaded on {backend}",
  "backend.unloaded": "Model unloaded; it reloads on the next search",
  "hint.open": "Open",
  "hint.reveal": "Show in folder",
  "hint.hide": "Hide",

  // Banners
  "banner.indexing": "Indexing your files. Some results may be missing until it finishes.",
  "banner.incomplete": "Indexing is not finished for {names}. Files that have not been indexed will not appear.",
  "banner.stop": "Stop",
  "banner.resume": "Resume",
  "tip.gpu": "An NVIDIA graphics card was found. GPU support makes indexing several times faster.",
  "tip.download": "Download ({size})",
  "tip.later": "Not now",
  "update.available": "NLsearch {version} is available.",
  "update.install": "Update and restart",
  "update.download": "Download",

  // Folders panel
  "drawer.intro": "NLsearch looks through these folders. Files stay on this computer.",
  "drawer.none": "No folders yet. Add one to start searching.",
  "drawer.add": "Add folder",
  "drawer.rescan": "Rescan",
  "drawer.rescanTitle": "Look for new, changed and deleted files",
  "drawer.close": "Close",
  "drawer.remove": "Stop searching this folder",
  "drawer.choose": "Choose a folder to search",
  "drop.hint": "Drop to add folder",
  "folder.files_one": "{n} file",
  "folder.files_other": "{n} files",
  "folder.waiting": "Waiting to be indexed",
  "folder.notIndexed": "Not indexed yet",
  "folder.toGo": "{files} indexed · {n} to go",
  "folder.missing": "{files} indexed · {n} not indexed",
  "folder.cloud_one": "{files} · {n} only in the cloud",
  "folder.cloud_other": "{files} · {n} only in the cloud",
  "add.started": "Indexing {names}. You can search while it finishes.",
  "add.notFolder": "Only folders can be added.",
  "add.covered": "Already searched as part of {folder}",

  // About
  "about.version": "Version {version}",
  "about.text": "Find documents and pictures on this computer by what they contain. Everything runs locally; no file leaves your device.",
  "about.model": "Model",
  "about.modelName": "EmbeddingGemma 2 (8-bit), by Google DeepMind",
  "about.runningOn": "Running on",
  "about.index": "Index",
  "about.license": "License",
  "about.licenseText": "MIT. See LICENSE",
  "about.notStarted": "Not started yet; it loads on the first search",
  "about.gpu": "GPU (NVIDIA CUDA)",
  "about.idle": "{device}, unloaded while idle",
  "about.indexSize": "{files} in {folders}",
  "about.folders_one": "{n} folder",
  "about.folders_other": "{n} folders",
  close: "Close",

  // Settings
  "set.appearance": "Appearance",
  "set.appearanceText": "Follow Windows, or always use a light or dark look.",
  "set.glass": "Glass",
  "set.paper": "Paper",
  "set.system": "System",
  "set.light": "Light",
  "set.dark": "Dark",
  "set.language": "Language",
  "set.languageText": "The language of NLsearch itself. Searching works in every language either way.",
  "set.languageSystem": "Same as Windows",
  "set.history": "History",
  "set.historyText": "Recent searches and opened files appear on the start screen. They are kept only on this computer.",
  "set.historyKeep": "Remember recent searches and files",
  "set.historyClear": "Clear history",
  "set.historyCleared": "History cleared.",
  "set.gpu": "Graphics acceleration",
  "set.gpuInstalled": "Installed. Searching and indexing use your NVIDIA graphics card.",
  "set.gpuFound": "An NVIDIA graphics card was found. Downloading GPU support makes indexing several times faster.",
  "set.gpuNone": "No NVIDIA graphics card was found, so NLsearch runs on the processor.",
  "set.gpuDownload": "Download GPU support ({size})",
  "set.model": "Search model",
  "set.modelInstalled": "Installed: EmbeddingGemma 2 (8-bit).",
  "set.modelMissing": "Not installed. NLsearch cannot search until the model is downloaded.",
  "set.modelDownload": "Download model ({size})",
  "set.startup": "Start-up",
  "set.autostart": "Start NLsearch in the tray when you sign in",
  "set.autostartFailed": "Could not change start-up: {error}",
  "set.ocr": "Text in pictures (OCR)",
  "set.ocrOn": "Read text in pictures and scanned PDFs",
  "set.ocrAvailable":
    "Finds screenshots and scanned documents by the words in them. Uses Windows' built-in OCR, with no download; indexing pictures takes longer and the index grows a little. Reads: {languages}.",
  "set.ocrMissing":
    "Windows has no OCR language installed. Add one under Windows Settings › Time & language › Language & region, in the language's options.",
  "set.ocrStarted": "Pictures and PDFs are being read again for text.",
  "set.ocrStopped": "Pictures and PDFs are being indexed again without OCR.",
  "set.cloud": "Cloud files (OneDrive)",
  "set.cloudText":
    "Files kept only in the cloud are left out, because reading one downloads it. What was indexed before a file went back to the cloud stays searchable. When on, files are downloaded one at a time, never on a metered connection, and each goes back to the cloud once read, unless you set it to always keep on this device.",
  "set.cloudOn": "Download cloud files to index them, then free up the space again",
  "set.cloudStarted": "Cloud files are being downloaded one at a time to be indexed.",
  "set.cloudStopped": "Files only in the cloud are left out from now on.",
  "set.exclude": "Skipped folders and files",
  "set.excludeText":
    "One per line: a name such as Archive, a pattern such as *.tmp, or a full path. Hidden folders and program folders such as node_modules are always skipped.",
  "set.excludeSaved": "Saved. Skipped files are leaving the index.",
  save: "Save",
  "save.failed": "Could not save: {error}",
  "set.updates": "Updates",
  "set.updateAuto": "Check for updates automatically",
  "set.updateCheck": "Check now",
  "set.updateAvailable": "Version {version} is available. You have {current}.",
  "set.updateLatest": "You have the latest version ({current}).",
  "set.updateVersion": "You have version {current}.",
  "update.checkFailed": "Could not check for updates: {error}",
  "update.failed": "The update failed: {error}",
  "update.downloading": "Downloading…",
  "update.percent": "Downloading {n}%",
  "update.installing": "Installing…",
  cancel: "Cancel",
  "download.progress": "{step} · {done} of {total}",
  "download.failed": "Download failed: {error}. Try again to continue where it stopped.",
  "download.gpuDone": "GPU support is installed.",
  "download.modelDone": "The model is installed.",
  "step.Starting": "Starting",
  "step.Installing": "Installing",
  "step.Downloading": "Downloading {file}",
  "step.Verifying": "Verifying {file}",
  megabytes: "{n} MB",

  // Tray menu
  "tray.show": "Show NLsearch",
  "tray.about": "About NLsearch",
  "tray.quit": "Quit",
};

type Key = keyof typeof en;

const zh: Record<Key, string> = {
  "search.placeholder": "用內容搜尋檔案，例如「房租多少」",
  "search.clear": "清除",
  "tab.all": "全部",
  "tab.docs": "文件",
  "tab.images": "圖片",
  folders: "資料夾",
  "folders.title": "選擇要搜尋的資料夾",
  "filters.title": "依日期、資料夾或檔案類型篩選",
  "settings.title": "設定",
  "about.title": "關於 NLsearch",
  "preview.toggle": "顯示或隱藏預覽",
  "sort.relevance": "最相符",
  "sort.newest": "最新的在前",
  "sort.oldest": "最舊的在前",
  "sort.title": "結果的排列順序",
  "meta.results_one": "{n} 筆結果 · {ms} 毫秒",
  "meta.results_other": "{n} 筆結果 · {ms} 毫秒",
  "similar.to": "與 {name} 相似",
  "similar.clear": "回到搜尋",

  "filter.anytime": "不限時間",
  "filter.day": "過去一天",
  "filter.week": "過去一週",
  "filter.month": "過去一個月",
  "filter.year": "過去一年",
  "filter.allFolders": "所有資料夾",
  "filter.allTypes": "所有檔案類型",
  "filter.clear": "清除",
  "type.pdf": "PDF",
  "type.word": "Word",
  "type.excel": "Excel 與 CSV",
  "type.powerpoint": "PowerPoint",
  "type.text": "文字檔與網頁",
  "type.code": "程式碼",
  "type.pictures": "圖片",

  "kind.pdf": "PDF",
  "kind.word": "Word 文件",
  "kind.csv": "CSV",
  "kind.web": "網頁",
  "kind.markdown": "Markdown",
  "kind.text": "文字檔",
  "kind.log": "記錄檔",
  "kind.excel": "Excel 活頁簿",
  "kind.powerpoint": "PowerPoint",
  "kind.code": "程式碼",
  "kind.image": "{ext} 圖片",
  page: "第 {n} 頁",
  slide: "第 {n} 張投影片",

  "match.exact": "完全相符",
  "match.strong": "非常相關",
  "match.good": "相關",
  "match.related": "可能相關",
  "match.similarity": "相似度 {score}",
  "action.open": "開啟",
  "action.reveal": "在資料夾中顯示",
  "action.similar": "找相似的",
  "action.copyPath": "複製路徑",
  "action.copyText": "複製內文",
  "copied.path": "已複製路徑。",
  "copied.text": "已複製內文。",
  "copy.failed": "無法複製：{error}",
  "open.failed": "無法開啟 {name}：{error}",
  more: "顯示更多結果",
  "noResults.title": "找不到結果",
  "noResults.detail": "換個說法，或改用其他篩選條件試試。",
  "noSimilar.detail": "索引裡沒有和這個檔案很像的其他檔案。",
  "searchFailed.title": "搜尋失敗",

  "preview.empty": "選一個結果，就會在這裡顯示內容。",
  "preview.loading": "載入中…",
  "preview.none": "這個檔案裡沒有找到文字。",
  "preview.truncated": "這是長檔案的開頭部分，開啟檔案可以看完整內容。",
  "preview.failed": "無法預覽：{error}",

  "idle.title": "搜尋你的檔案",
  "idle.detail": "用內容找文件和圖片，也可以直接輸入檔名或號碼。",
  "idle.recentSearches": "最近的搜尋",
  "idle.recentFiles": "最近開啟",
  "idle.forget": "從紀錄中移除",
  "idle.try": "試試看",
  "suggest.1": "有總金額的發票",
  "suggest.2": "夕陽的照片",
  "suggest.3": "wifi 密碼",
  "suggest.4": "租屋合約",
  "addFolder.title": "加入要搜尋的資料夾",
  "addFolder.detail": "NLsearch 會讀取你選的資料夾裡的文件和圖片，讓你能用內容找到它們。",
  "addFolder.drag": "也可以直接把資料夾拖進來。",
  "setup.title": "下載一次就能開始",
  "setup.detailGpu":
    "NLsearch 需要搜尋模型。偵測到 NVIDIA 顯示卡，所以會一起下載 GPU 支援，讓建立索引快好幾倍。所有運算都在這台電腦上進行。",
  "setup.detail": "NLsearch 需要搜尋模型。只要下載一次，之後完全在這台電腦上執行。",
  "setup.both": "下載模型與 GPU 支援（{size}）",
  "setup.model": "下載模型（{size}）",
  "setup.modelOnly": "只下載模型（{size}）",

  "status.indexed_one": "已建立 {n} 個檔案的索引",
  "status.indexed_other": "已建立 {n} 個檔案的索引",
  "status.progress": "{phase} {done} / {total}",
  "status.error": "建立索引時發生錯誤：{error}",
  "phase.Scanning": "掃描中",
  "phase.Loading model": "載入模型中",
  "phase.Indexing": "建立索引中",
  "backend.gpu": "GPU",
  "backend.cpuGpu": "CPU · 可使用 GPU",
  "backend.loaded": "模型已載入 {backend}",
  "backend.unloaded": "模型閒置時已卸載，下次搜尋會重新載入",
  "hint.open": "開啟",
  "hint.reveal": "在資料夾中顯示",
  "hint.hide": "隱藏",

  "banner.indexing": "正在建立檔案索引，完成之前可能會漏掉部分結果。",
  "banner.incomplete": "{names} 的索引還沒建立完成，尚未建立索引的檔案不會出現在結果中。",
  "banner.stop": "停止",
  "banner.resume": "繼續",
  "tip.gpu": "偵測到 NVIDIA 顯示卡。加裝 GPU 支援能讓建立索引快好幾倍。",
  "tip.download": "下載（{size}）",
  "tip.later": "以後再說",
  "update.available": "NLsearch {version} 已推出。",
  "update.install": "更新並重新啟動",
  "update.download": "下載",

  "drawer.intro": "NLsearch 會搜尋這些資料夾，檔案不會離開這台電腦。",
  "drawer.none": "還沒有資料夾，加入一個就能開始搜尋。",
  "drawer.add": "加入資料夾",
  "drawer.rescan": "重新掃描",
  "drawer.rescanTitle": "尋找新增、修改和刪除的檔案",
  "drawer.close": "關閉",
  "drawer.remove": "不再搜尋這個資料夾",
  "drawer.choose": "選擇要搜尋的資料夾",
  "drop.hint": "放開以加入資料夾",
  "folder.files_one": "{n} 個檔案",
  "folder.files_other": "{n} 個檔案",
  "folder.waiting": "等待建立索引",
  "folder.notIndexed": "尚未建立索引",
  "folder.toGo": "已完成 {files} · 還剩 {n} 個",
  "folder.missing": "已完成 {files} · {n} 個尚未建立索引",
  "folder.cloud_one": "{files} · {n} 個僅在雲端",
  "folder.cloud_other": "{files} · {n} 個僅在雲端",
  "add.started": "正在為 {names} 建立索引，完成前也可以先搜尋。",
  "add.notFolder": "只能加入資料夾。",
  "add.covered": "已經包含在 {folder} 裡搜尋了",

  "about.version": "版本 {version}",
  "about.text": "用內容找到這台電腦上的文件和圖片。所有處理都在本機進行，檔案不會離開你的裝置。",
  "about.model": "模型",
  "about.modelName": "EmbeddingGemma 2（8 位元），Google DeepMind 開發",
  "about.runningOn": "執行於",
  "about.index": "索引",
  "about.license": "授權",
  "about.licenseText": "MIT，詳見 LICENSE",
  "about.notStarted": "尚未啟動，第一次搜尋時載入",
  "about.gpu": "GPU（NVIDIA CUDA）",
  "about.idle": "{device}，閒置時已卸載",
  "about.indexSize": "{folders}中的 {files}",
  "about.folders_one": "{n} 個資料夾",
  "about.folders_other": "{n} 個資料夾",
  close: "關閉",

  "set.appearance": "外觀",
  "set.appearanceText": "跟隨 Windows，或固定使用淺色或深色。",
  "set.glass": "玻璃",
  "set.paper": "紙張",
  "set.system": "跟隨系統",
  "set.light": "淺色",
  "set.dark": "深色",
  "set.language": "語言",
  "set.languageText": "NLsearch 介面使用的語言。不論選哪一種，都能搜尋任何語言的內容。",
  "set.languageSystem": "與 Windows 相同",
  "set.history": "紀錄",
  "set.historyText": "最近的搜尋和開啟過的檔案會顯示在首頁，只保存在這台電腦上。",
  "set.historyKeep": "記住最近的搜尋和檔案",
  "set.historyClear": "清除紀錄",
  "set.historyCleared": "紀錄已清除。",
  "set.gpu": "圖形加速",
  "set.gpuInstalled": "已安裝。搜尋和建立索引都會使用你的 NVIDIA 顯示卡。",
  "set.gpuFound": "偵測到 NVIDIA 顯示卡。下載 GPU 支援能讓建立索引快好幾倍。",
  "set.gpuNone": "沒有偵測到 NVIDIA 顯示卡，NLsearch 會使用處理器執行。",
  "set.gpuDownload": "下載 GPU 支援（{size}）",
  "set.model": "搜尋模型",
  "set.modelInstalled": "已安裝：EmbeddingGemma 2（8 位元）。",
  "set.modelMissing": "尚未安裝。下載模型之前無法搜尋。",
  "set.modelDownload": "下載模型（{size}）",
  "set.startup": "開機啟動",
  "set.autostart": "登入 Windows 時在系統匣啟動 NLsearch",
  "set.autostartFailed": "無法變更開機啟動：{error}",
  "set.ocr": "圖片中的文字（OCR）",
  "set.ocrOn": "讀取圖片和掃描版 PDF 裡的文字",
  "set.ocrAvailable":
    "讓截圖和掃描文件也能用裡面的文字找到。使用 Windows 內建的 OCR，不需要另外下載；建立圖片索引會比較久，索引也會稍微變大。可讀取：{languages}。",
  "set.ocrMissing": "Windows 尚未安裝 OCR 語言。請到 Windows 設定 › 時間與語言 › 語言與地區，在語言選項中加裝。",
  "set.ocrStarted": "正在重新讀取圖片和 PDF 裡的文字。",
  "set.ocrStopped": "正在不使用 OCR 重新建立圖片和 PDF 的索引。",
  "set.cloud": "雲端檔案（OneDrive）",
  "set.cloudText":
    "只存在雲端的檔案預設略過，因為一讀取就會下載。檔案變回僅雲端之前建立的索引會保留，仍然搜得到。開啟後會一次下載一個檔案，使用計量付費網路時不下載；讀完就恢復成僅雲端，設成「永遠保留在此裝置上」的檔案除外。",
  "set.cloudOn": "下載雲端檔案來建立索引，完成後恢復成僅雲端",
  "set.cloudStarted": "正在逐一下載雲端檔案來建立索引。",
  "set.cloudStopped": "之後會略過只存在雲端的檔案。",
  "set.exclude": "略過的資料夾和檔案",
  "set.excludeText":
    "每行一個：名稱（例如 Archive）、樣式（例如 *.tmp）或完整路徑。隱藏資料夾和 node_modules 這類程式資料夾一律會略過。",
  "set.excludeSaved": "已儲存，略過的檔案正從索引中移除。",
  save: "儲存",
  "save.failed": "無法儲存：{error}",
  "set.updates": "更新",
  "set.updateAuto": "自動檢查更新",
  "set.updateCheck": "立即檢查",
  "set.updateAvailable": "版本 {version} 已推出，你目前使用 {current}。",
  "set.updateLatest": "你使用的是最新版本（{current}）。",
  "set.updateVersion": "你目前使用版本 {current}。",
  "update.checkFailed": "無法檢查更新：{error}",
  "update.failed": "更新失敗：{error}",
  "update.downloading": "下載中…",
  "update.percent": "下載中 {n}%",
  "update.installing": "安裝中…",
  cancel: "取消",
  "download.progress": "{step} · {done} / {total}",
  "download.failed": "下載失敗：{error}。再試一次會從中斷的地方繼續。",
  "download.gpuDone": "GPU 支援已安裝。",
  "download.modelDone": "模型已安裝。",
  "step.Starting": "準備中",
  "step.Installing": "安裝中",
  "step.Downloading": "正在下載 {file}",
  "step.Verifying": "正在驗證 {file}",
  megabytes: "{n} MB",

  "tray.show": "顯示 NLsearch",
  "tray.about": "關於 NLsearch",
  "tray.quit": "結束",
};

const TEXTS: Record<Language, Record<Key, string>> = { en, "zh-TW": zh };

export function languageChoice(): LanguageChoice {
  try {
    const saved = localStorage.getItem("language");
    if (saved === "en" || saved === "zh-TW") return saved;
  } catch {
    // Without storage the Windows language applies.
  }
  return "system";
}

/** Chinese in any form gets Traditional Chinese; everything else English. */
function resolve(choice: LanguageChoice): Language {
  if (choice !== "system") return choice;
  return navigator.languages.some((l) => l.toLowerCase().startsWith("zh")) ? "zh-TW" : "en";
}

export const lang: Language = resolve(languageChoice());

export function setLanguageChoice(choice: LanguageChoice) {
  try {
    if (choice === "system") localStorage.removeItem("language");
    else localStorage.setItem("language", choice);
  } catch {
    // The choice lasts until NLsearch closes.
  }
}

export function t(key: Key, vars: Record<string, string | number> = {}): string {
  const text = TEXTS[lang][key] ?? en[key];
  return text.replace(/\{(\w+)\}/g, (_, name) => {
    const v = vars[name];
    return v === undefined ? "" : typeof v === "number" ? v.toLocaleString(lang) : v;
  });
}

/** A text with a count in it: `${key}_one` for one, `${key}_other` otherwise. */
export function tn(key: string, n: number, vars: Record<string, string | number> = {}) {
  return t(`${key}_${n === 1 ? "one" : "other"}` as Key, { n, ...vars });
}

/** Has the key, for texts the backend sends that may or may not be known here. */
export function has(key: string): key is Key {
  return key in en;
}

/**
 * Fills the static page: data-i18n sets the text, data-i18n-title the tooltip,
 * data-i18n-placeholder the placeholder.
 */
export function translatePage() {
  document.documentElement.lang = lang;
  for (const node of document.querySelectorAll<HTMLElement>("[data-i18n]")) {
    node.textContent = t(node.dataset.i18n as Key);
  }
  for (const node of document.querySelectorAll<HTMLElement>("[data-i18n-title]")) {
    node.title = t(node.dataset.i18nTitle as Key);
  }
  for (const node of document.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>("[data-i18n-placeholder]")) {
    node.placeholder = t(node.dataset.i18nPlaceholder as Key);
  }
}

export type TextKey = Key;
