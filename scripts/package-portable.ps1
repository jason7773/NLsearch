# Builds a portable copy of NLsearch: one folder that runs from anywhere (USB stick,
# network share) without installation and keeps its index beside the executable.
#
#   pwsh scripts\package-portable.ps1            GPU (CUDA) and CPU backends
#   pwsh scripts\package-portable.ps1 -CpuOnly   CPU backend only, much smaller
#   pwsh scripts\package-portable.ps1 -Lite      CPU backend and no model: a small download;
#                                                the app fetches the model, and GPU support
#                                                if wanted, on first run
#   pwsh scripts\package-portable.ps1 -Zip       also produce a .zip of the folder
#   pwsh scripts\package-portable.ps1 -SkipBuild use the executable already in target\release,
#                                                built with RUSTFLAGS=-C target-feature=+crt-static
param(
    [switch]$CpuOnly,
    [switch]$Lite,
    [switch]$Zip,
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
$runtime = Join-Path $root 'runtime'
$name = if ($Lite) { 'NLsearch-lite' } elseif ($CpuOnly) { 'NLsearch-cpu' } else { 'NLsearch' }
$out = Join-Path $root "dist-portable\$name"

if (-not $SkipBuild) {
    # Link the C runtime statically so the executable needs no Visual C++ redistributable.
    $env:RUSTFLAGS = '-C target-feature=+crt-static'
    Push-Location $root
    try {
        pnpm tauri build --no-bundle
        if ($LASTEXITCODE -ne 0) { throw 'build failed' }
    } finally {
        Pop-Location
        Remove-Item Env:RUSTFLAGS
    }
}

if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force $out, "$out\runtime\models" | Out-Null
Copy-Item (Join-Path $root 'src-tauri\target\release\nlsearch.exe') "$out\NLsearch.exe"

# Of each llama.cpp build only the server and its libraries are needed.
$backends = if ($CpuOnly -or $Lite) { @('llama-cpu') } else { @('llama-cuda', 'llama-cpu') }
foreach ($backend in $backends) {
    $from = Join-Path $runtime $backend
    if (-not (Test-Path "$from\llama-server.exe")) { throw "missing $from\llama-server.exe" }
    New-Item -ItemType Directory -Force "$out\runtime\$backend" | Out-Null
    Copy-Item "$from\llama-server.exe", "$from\*.dll" "$out\runtime\$backend"
}
if (-not $Lite) {
    Copy-Item "$runtime\models\embeddinggemma-2-Q8_0.gguf", "$runtime\models\mmproj-embeddinggemma-2-Q8_0.gguf" "$out\runtime\models"
}

# The presence of this folder is what switches the app to portable mode.
New-Item -ItemType Directory -Force "$out\data" | Out-Null
Copy-Item (Join-Path $root 'LICENSE') "$out\LICENSE.txt"
@"
NLsearch (portable)

Run NLsearch.exe. Nothing is installed and nothing is written outside this folder:
the index, logs and settings are kept in the "data" folder. Delete "data" to reset.

Requires 64-bit Windows 10 or 11 with the Microsoft Edge WebView2 Runtime, which is
part of Windows 11 and of up-to-date Windows 10.
$(if ($Lite) { 'On first run the app downloads its search model (about 825 MB). GPU support for NVIDIA cards is an optional download in Settings.' } elseif ($CpuOnly) { 'This copy runs the model on the CPU. GPU support for NVIDIA cards is an optional download in Settings.' } else { 'An NVIDIA GPU is used when present; otherwise the model runs on the CPU.' })

Folders you add are remembered by their full path, so on another PC add them again.
"@ | Set-Content "$out\README.txt"

$size = (Get-ChildItem $out -Recurse -File | Measure-Object Length -Sum).Sum
"Portable app: $out ($([math]::Round($size / 1GB, 2)) GB)"

if ($Zip) {
    $archive = "$out.zip"
    if (Test-Path $archive) { Remove-Item $archive }
    # Windows' tar writes zip archives and has no 2 GB limit, unlike Compress-Archive.
    tar.exe -a -c -f $archive -C (Split-Path $out) $name
    if ($LASTEXITCODE -ne 0) { throw 'zip failed' }
    "Archive: $archive ($([math]::Round((Get-Item $archive).Length / 1GB, 2)) GB)"
}
