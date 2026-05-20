#!/usr/bin/env bash
set -euo pipefail

detect_script_path() {
  if [[ -n "${BASH_SOURCE:-}" ]]; then
    printf '%s\n' "${BASH_SOURCE[0]}"
    return
  fi

  if [[ -n "${ZSH_VERSION:-}" ]]; then
    printf '%s\n' "${(%):-%x}"
    return
  fi

  printf '%s\n' "$0"
}

SCRIPT_PATH="$(detect_script_path)"
SCRIPT_DIR="$(cd "$(dirname "$SCRIPT_PATH")" && pwd -P)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd -P)"

if [[ ! -f "$ROOT_DIR/Cargo.toml" ]]; then
  cat >&2 <<EOF
error: could not locate the project root from script path:
  $SCRIPT_PATH

Run this script from the project root with:
  ./scripts/setup-embedded-vision-pi.sh
EOF
  exit 1
fi

MODEL_NAME="face_detection_yunet_2023mar.onnx"
MODEL_URL="${MODEL_URL:-https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/face_detection_yunet/${MODEL_NAME}}"
MODELS_DIR="${MODELS_DIR:-$ROOT_DIR/config/models}"
VISION_MODEL_DIR="${VISION_MODEL_DIR:-$MODELS_DIR/vision}"
MODEL_PATH="${MODEL_PATH:-$VISION_MODEL_DIR/$MODEL_NAME}"
CAMERA_INDEX="${CAMERA_INDEX:-0}"
CONFIG_PATH="${ALIA_EMBEDDED_CONFIG_PATH:-$ROOT_DIR/config/embedded.toml}"
SKIP_DEPS=0
SKIP_MODEL=0
SKIP_CHECK=0
RUN_AFTER=0

usage() {
  cat <<EOF
Usage: $0 [options]

Prepare Raspberry Pi 5 for alia-embedded YuNet/OpenCV face tracking.

Options:
  --camera-index N      Camera index to probe (default: $CAMERA_INDEX)
  --config PATH         Embedded config path for printed run command
                        (default: $CONFIG_PATH)
  --model-path PATH     YuNet model output path
                        (default: $MODEL_PATH)
  --model-url URL       YuNet ONNX download URL
                        (default: $MODEL_URL)
  --skip-deps           Do not install distro packages
  --skip-model          Do not download the YuNet model
  --skip-cargo-check    Do not run cargo check with vision-opencv
  --run                 Start alia-embedded after setup
  -h, --help            Show this help

Environment overrides: CAMERA_INDEX, ALIA_EMBEDDED_CONFIG_PATH, MODEL_URL,
MODELS_DIR, VISION_MODEL_DIR, MODEL_PATH, ALLOW_OLD_OPENCV=1
EOF
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1
}

install_deps() {
  if [[ "$SKIP_DEPS" -eq 1 ]]; then
    return
  fi

  if ! need_cmd apt-get; then
    cat >&2 <<EOF
warning: apt-get was not found.
Install these packages manually, then rerun with --skip-deps:
  build-essential clang libclang-dev pkg-config cmake curl ca-certificates libopencv-dev v4l-utils
EOF
    return
  fi

  sudo apt-get update
  sudo apt-get install -y \
    build-essential \
    ca-certificates \
    clang \
    cmake \
    curl \
    libclang-dev \
    libopencv-dev \
    pkg-config \
    v4l-utils
}

download_model() {
  if [[ "$SKIP_MODEL" -eq 1 ]]; then
    validate_model_file
    return
  fi
  if [[ -f "$MODEL_PATH" ]]; then
    if validate_model_file; then
      echo "Using existing YuNet model: $MODEL_PATH"
      return
    fi
    echo "Existing YuNet model is invalid; re-downloading: $MODEL_PATH"
    rm -f "$MODEL_PATH"
  fi

  mkdir -p "$(dirname "$MODEL_PATH")"
  echo "Downloading YuNet model:"
  echo "  $MODEL_URL"
  curl -L --fail --retry 3 --output "$MODEL_PATH" "$MODEL_URL"
  validate_model_file
}

validate_model_file() {
  if [[ ! -f "$MODEL_PATH" ]]; then
    if [[ "$SKIP_MODEL" -eq 1 ]]; then
      cat >&2 <<EOF
error: YuNet model does not exist:
  $MODEL_PATH

Rerun without --skip-model, or pass --model-path to an existing ONNX file.
EOF
      exit 1
    fi
    return
  fi

  local size
  size="$(wc -c < "$MODEL_PATH" | tr -d ' ')"
  local head
  head="$(head -c 128 "$MODEL_PATH" || true)"

  if [[ "$head" == version\ https://git-lfs.github.com/spec/v1* ]]; then
    cat >&2 <<EOF
error: YuNet model is a Git LFS pointer, not the ONNX payload:
  $MODEL_PATH

Use the GitHub media URL or Hugging Face raw URL instead of raw.githubusercontent.com.
Current default:
  https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/face_detection_yunet/$MODEL_NAME
EOF
    return 1
  fi

  if [[ "$head" == \<html* || "$head" == \<\!DOCTYPE* ]]; then
    cat >&2 <<EOF
error: YuNet model path contains HTML, not an ONNX payload:
  $MODEL_PATH
EOF
    return 1
  fi

  if [[ "$size" -lt 100000 ]]; then
    cat >&2 <<EOF
error: YuNet model file is unexpectedly small (${size} bytes):
  $MODEL_PATH

The expected face_detection_yunet_2023mar.onnx payload is roughly 230 KB.
EOF
    return 1
  fi

  echo "YuNet model validated: $MODEL_PATH (${size} bytes)"
}

check_opencv() {
  local version=""
  if pkg-config --modversion opencv4 >/dev/null 2>&1; then
    version="$(pkg-config --modversion opencv4)"
    echo "OpenCV pkg-config: opencv4 $version"
    check_opencv_version "$version"
    return
  fi
  if pkg-config --modversion opencv >/dev/null 2>&1; then
    version="$(pkg-config --modversion opencv)"
    echo "OpenCV pkg-config: opencv $version"
    check_opencv_version "$version"
    return
  fi

  cat >&2 <<EOF
error: pkg-config cannot find OpenCV.

Expected one of:
  pkg-config --modversion opencv4
  pkg-config --modversion opencv

If OpenCV is installed in a custom prefix, set PKG_CONFIG_PATH to the directory
that contains opencv4.pc.
EOF
  exit 1
}

check_opencv_version() {
  local version="$1"
  local clean="${version%%[-+~]*}"
  local major=0
  local minor=0
  IFS=. read -r major minor _ <<<"$clean"
  major="${major:-0}"
  minor="${minor:-0}"

  if [[ "${ALLOW_OLD_OPENCV:-0}" == "1" ]]; then
    return
  fi

  if (( major < 4 || (major == 4 && minor < 8) )); then
    cat >&2 <<EOF
error: OpenCV $version is too old for the default YuNet 2023mar model.

OpenCV 4.6/4.7 can load the model, then fail at runtime with:
  Layer with requested id=-1 not found

Install OpenCV 4.8 or newer, or set MODEL_PATH/MODEL_URL to an older YuNet
model compatible with your OpenCV build. To bypass this check intentionally:
  ALLOW_OLD_OPENCV=1 $0 ...
EOF
    exit 1
  fi
}

check_camera() {
  local device="/dev/video${CAMERA_INDEX}"
  if [[ ! -e "$device" ]]; then
    cat >&2 <<EOF
warning: $device does not exist.
Check camera connection and Raspberry Pi camera configuration before runtime.
EOF
    return
  fi

  echo "Camera device exists: $device"
  if need_cmd v4l2-ctl; then
    v4l2-ctl --device "$device" --all >/dev/null || {
      cat >&2 <<EOF
warning: v4l2-ctl could not query $device.
The camera may still work, but verify permissions and driver state.
EOF
    }
  fi
}

run_cargo_check() {
  if [[ "$SKIP_CHECK" -eq 1 ]]; then
    return
  fi
  (
    cd "$ROOT_DIR"
    cargo check -p alia-embedded --features vision-opencv
  )
}

print_next_steps() {
  cat <<EOF

YuNet/OpenCV setup is ready.

Model:
  $MODEL_PATH

Config defaults expect:
  config/models/vision/$MODEL_NAME

Run alia-embedded with YuNet vision:
  ALIA_EMBEDDED_CONFIG_PATH="$CONFIG_PATH" \\
  RUST_LOG=alia_embedded::module::vision=debug,alia_embedded::module::motion=debug,info \\
  cargo run -p alia-embedded --features vision-opencv

If OpenCV is installed outside the system pkg-config path, export PKG_CONFIG_PATH
before running cargo.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --camera-index) CAMERA_INDEX="$2"; shift 2 ;;
    --config) CONFIG_PATH="$2"; shift 2 ;;
    --model-path) MODEL_PATH="$2"; shift 2 ;;
    --model-url) MODEL_URL="$2"; shift 2 ;;
    --skip-deps) SKIP_DEPS=1; shift ;;
    --skip-model) SKIP_MODEL=1; shift ;;
    --skip-cargo-check) SKIP_CHECK=1; shift ;;
    --run) RUN_AFTER=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

install_deps
download_model
check_opencv
check_camera
run_cargo_check
print_next_steps

if [[ "$RUN_AFTER" -eq 1 ]]; then
  cd "$ROOT_DIR"
  export ALIA_EMBEDDED_CONFIG_PATH="$CONFIG_PATH"
  export RUST_LOG="${RUST_LOG:-alia_embedded::module::vision=debug,alia_embedded::module::motion=debug,info}"
  exec cargo run -p alia-embedded --features vision-opencv
fi
