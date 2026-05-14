param(
    [string]$Model = $(if ($env:MODEL) { $env:MODEL } else { "large-v3-turbo" }),
    [string]$Prefix = $(if ($env:WHISPER_CPP_DIR) { $env:WHISPER_CPP_DIR } else { Join-Path $env:LOCALAPPDATA "Alia\whisper.cpp" }),
    [string]$HostName = $(if ($env:HOST) { $env:HOST } else { "127.0.0.1" }),
    [int]$Port = $(if ($env:PORT) { [int]$env:PORT } else { 8080 }),
    [int]$Threads = $(if ($env:THREADS) { [int]$env:THREADS } else { 4 }),
    [switch]$Cuda,
    [switch]$SkipDeps,
    [switch]$Start
)

$ErrorActionPreference = "Stop"

function Has-Command($Name) {
    return $null -ne (Get-Command $Name -ErrorAction SilentlyContinue)
}

function Install-MissingDeps {
    $missing = @()
    foreach ($cmd in @("git", "cmake")) {
        if (-not (Has-Command $cmd)) {
            $missing += $cmd
        }
    }

    if ($missing.Count -eq 0) {
        return
    }
    if ($SkipDeps) {
        throw "Missing required commands: $($missing -join ', ')"
    }
    if (-not (Has-Command "winget")) {
        throw "Missing $($missing -join ', ') and winget is unavailable. Install Git and CMake, then rerun with -SkipDeps."
    }

    if ($missing -contains "git") {
        winget install --id Git.Git -e --source winget
    }
    if ($missing -contains "cmake") {
        winget install --id Kitware.CMake -e --source winget
    }
}

function Download-Model {
    $modelsDir = Join-Path $Prefix "models"
    New-Item -ItemType Directory -Force -Path $modelsDir | Out-Null
    $modelPath = Join-Path $modelsDir "ggml-$Model.bin"

    if (Test-Path $modelPath) {
        Write-Host "Using existing model: $modelPath"
        return $modelPath
    }

    $url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-$Model.bin"
    Write-Host "Downloading $url"
    Invoke-WebRequest -Uri $url -OutFile $modelPath
    return $modelPath
}

Install-MissingDeps

$parent = Split-Path -Parent $Prefix
New-Item -ItemType Directory -Force -Path $parent | Out-Null

if (Test-Path (Join-Path $Prefix ".git")) {
    Write-Host "Using existing whisper.cpp checkout: $Prefix"
} else {
    git clone https://github.com/ggml-org/whisper.cpp.git $Prefix
}

$cmakeFlags = @("-DCMAKE_BUILD_TYPE=Release")
if ($Cuda) {
    $cmakeFlags += "-DGGML_CUDA=ON"
}

cmake -S $Prefix -B (Join-Path $Prefix "build") @cmakeFlags
cmake --build (Join-Path $Prefix "build") --config Release

$modelPath = Download-Model
$candidateBins = @(
    (Join-Path $Prefix "build\bin\Release\whisper-server.exe"),
    (Join-Path $Prefix "build\bin\whisper-server.exe")
)
$serverBin = $candidateBins | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $serverBin) {
    throw "Could not find whisper-server.exe under $Prefix\build\bin"
}

Write-Host ""
Write-Host "whisper.cpp STT is ready."
Write-Host ""
Write-Host "Start server:"
Write-Host "  `"$serverBin`" -m `"$modelPath`" --host $HostName --port $Port -t $Threads"
Write-Host ""
Write-Host "Health check:"
Write-Host "  curl http://$HostName`:$Port/health"
Write-Host ""
Write-Host "Alia config:"
Write-Host "  whisper_cpp_endpoint = `"http://$HostName`:$Port`""
Write-Host "  whisper_cpp_model = `"$Model`""

if ($Start) {
    & $serverBin -m $modelPath --host $HostName --port $Port -t $Threads
}
