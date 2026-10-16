# Runs the Traditional Chinese search evaluation against the real model: starts
# llama-server from NLsearch's runtime folder with the same settings as the app, runs
# the `search_eval` example against it, and stops the server again.
#
#   powershell -ExecutionPolicy Bypass -File scripts\eval-zh.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\eval-zh.ps1 -Runtime D:\NLsearch\runtime -Cpu
#
# Needs the Rust toolchain. Results are printed and saved under eval\results\.

param(
    # The runtime folder holding llama-cuda / llama-cpu and models (see README).
    [string]$Runtime = "$env:APPDATA\com.jason7773.nlsearch\runtime",
    # Use the CPU build even when the CUDA one is present.
    [switch]$Cpu,
    # Synthetic index size for the vector-scan timing.
    [int]$Scale = 100000
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot

$model = Join-Path $Runtime 'models\embeddinggemma-2-Q8_0.gguf'
$mmproj = Join-Path $Runtime 'models\mmproj-embeddinggemma-2-Q8_0.gguf'
if (-not (Test-Path $model) -or -not (Test-Path $mmproj)) {
    throw "Model files not found under $Runtime\models. Pass -Runtime <folder>."
}
$cuda = Join-Path $Runtime 'llama-cuda\llama-server.exe'
$cpuExe = Join-Path $Runtime 'llama-cpu\llama-server.exe'
if (-not $Cpu -and (Test-Path $cuda)) { $exe = $cuda; $ngl = '99'; $backend = 'cuda' }
elseif (Test-Path $cpuExe) { $exe = $cpuExe; $ngl = '0'; $backend = 'cpu' }
else { throw "llama-server.exe not found under $Runtime" }

# A free port, the way the app picks one.
$listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
$listener.Start(); $port = $listener.LocalEndpoint.Port; $listener.Stop()

$logDir = Join-Path $repo 'eval\results'
New-Item -ItemType Directory -Force $logDir | Out-Null
$log = Join-Path $logDir "llama-$backend.log"
Write-Host "Starting $exe on port $port ($backend)"
# Same arguments as src-tauri/src/engine.rs.
$server = Start-Process -FilePath $exe -PassThru -WindowStyle Hidden -RedirectStandardError $log -ArgumentList @(
    '-m', "`"$model`"", '--mmproj', "`"$mmproj`"", '--embeddings', '--pooling', 'mean',
    '-c', '2048', '-b', '2048', '-ub', '2048', '-ngl', $ngl, '--cache-ram', '0',
    '--host', '127.0.0.1', '--port', "$port")

try {
    $deadline = (Get-Date).AddMinutes(3)
    while ($true) {
        if ($server.HasExited) { throw "llama-server exited during startup, see $log" }
        try { Invoke-WebRequest "http://127.0.0.1:$port/health" -UseBasicParsing -TimeoutSec 2 | Out-Null; break } catch {}
        if ((Get-Date) -gt $deadline) { throw 'llama-server did not become ready in time' }
        Start-Sleep -Milliseconds 500
    }

    $stamp = Get-Date -Format 'yyyyMMdd-HHmm'
    $out = Join-Path $logDir "zh-tw-$backend-$stamp.json"
    Push-Location (Join-Path $repo 'src-tauri')
    try {
        cargo run --release -p nlsearch-core --example search_eval -- --server "http://127.0.0.1:$port" --scale $Scale --out $out
        if ($LASTEXITCODE -ne 0) { throw "search_eval failed ($LASTEXITCODE)" }
    } finally { Pop-Location }
} finally {
    if (-not $server.HasExited) { Stop-Process -Id $server.Id -Force }
}
