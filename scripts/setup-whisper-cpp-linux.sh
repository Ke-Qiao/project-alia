#!/usr/bin/env bash
set -euo pipefail

MODEL="${MODEL:-large-v3-turbo}"
PREFIX="${WHISPER_CPP_DIR:-$HOME/.local/share/alia/whisper.cpp}"
HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-8080}"
THREADS="${THREADS:-4}"
SKIP_DEPS=0
CUDA=0
START=0

usage() {
  cat <<EOF
Usage: $0 [options]

Build and prepare native whisper.cpp STT for Alia on Linux.

Options:
  --model NAME       Whisper model to download (default: $MODEL)
  --prefix DIR      whisper.cpp checkout/build directory (default: $PREFIX)
  --host HOST       whisper-server host for printed start command (default: $HOST)
  --port PORT       whisper-server port for printed start command (default: $PORT)
  --threads N       whisper-server worker threads (default: $THREADS)
  --cuda            Build with CUDA support (requires NVIDIA CUDA toolchain)
  --skip-deps       Do not install missing distro packages
  --start           Start whisper-server after setup
  -h, --help        Show this help

Environment overrides: MODEL, WHISPER_CPP_DIR, HOST, PORT, THREADS
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --model) MODEL="$2"; shift 2 ;;
    --prefix) PREFIX="$2"; shift 2 ;;
    --host) HOST="$2"; shift 2 ;;
    --port) PORT="$2"; shift 2 ;;
    --threads) THREADS="$2"; shift 2 ;;
    --cuda) CUDA=1; shift ;;
    --skip-deps) SKIP_DEPS=1; shift ;;
    --start) START=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

need_cmd() {
  command -v "$1" >/dev/null 2>&1
}

install_deps() {
  local missing=()
  for cmd in git cmake curl; do
    if ! need_cmd "$cmd"; then
      missing+=("$cmd")
    fi
  done

  if [[ ${#missing[@]} -eq 0 ]]; then
    return
  fi
  if [[ "$SKIP_DEPS" -eq 1 ]]; then
    echo "missing required commands: ${missing[*]}" >&2
    exit 1
  fi

  if need_cmd apt-get; then
    sudo apt-get update
    sudo apt-get install -y git cmake build-essential curl
  elif need_cmd dnf; then
    sudo dnf install -y git cmake gcc-c++ make curl
  elif need_cmd pacman; then
    sudo pacman -Sy --needed git cmake base-devel curl
  else
    echo "unsupported package manager; install git, cmake, a C++ compiler, and curl, then rerun --skip-deps" >&2
    exit 1
  fi
}

download_model() {
  local model_path="$PREFIX/models/ggml-${MODEL}.bin"
  if [[ -f "$model_path" ]]; then
    echo "Using existing model: $model_path"
    return
  fi

  if [[ -x "$PREFIX/models/download-ggml-model.sh" ]]; then
    "$PREFIX/models/download-ggml-model.sh" "$MODEL"
    return
  fi

  curl -L --fail --retry 3 \
    -o "$model_path" \
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-${MODEL}.bin"
}

install_deps

mkdir -p "$(dirname "$PREFIX")"
if [[ -d "$PREFIX/.git" ]]; then
  echo "Using existing whisper.cpp checkout: $PREFIX"
else
  git clone https://github.com/ggml-org/whisper.cpp.git "$PREFIX"
fi

CMAKE_FLAGS=(-DCMAKE_BUILD_TYPE=Release)
if [[ "$CUDA" -eq 1 ]]; then
  CMAKE_FLAGS+=(-DGGML_CUDA=ON)
else
  CMAKE_FLAGS+=(-DGGML_NATIVE=ON)
fi

cmake -S "$PREFIX" -B "$PREFIX/build" "${CMAKE_FLAGS[@]}"
cmake --build "$PREFIX/build" -j"$(nproc 2>/dev/null || echo 4)"

download_model

SERVER_BIN="$PREFIX/build/bin/whisper-server"
MODEL_PATH="$PREFIX/models/ggml-${MODEL}.bin"

cat <<EOF

whisper.cpp STT is ready.

Start server:
  "$SERVER_BIN" -m "$MODEL_PATH" --host "$HOST" --port "$PORT" -t "$THREADS"

Health check:
  curl http://$HOST:$PORT/health

Alia config (config/server.d/providers/whisper_cpp.toml):
  [providers.whisper_cpp]
  endpoint = "http://$HOST:$PORT"
  model = "$MODEL"
EOF

if [[ "$START" -eq 1 ]]; then
  exec "$SERVER_BIN" -m "$MODEL_PATH" --host "$HOST" --port "$PORT" -t "$THREADS"
fi
