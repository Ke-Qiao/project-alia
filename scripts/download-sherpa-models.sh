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

Run this script from the project root with either:
  ./scripts/download-sherpa-models.sh
  bash scripts/download-sherpa-models.sh
  zsh scripts/download-sherpa-models.sh
EOF
  exit 1
fi

MODELS_DIR="${MODELS_DIR:-$ROOT_DIR/config/models}"

case "${1:-}" in
  "") [[ $# -eq 0 ]] || exit 1 ;;
  --vad-only) [[ $# -eq 1 ]] || exit 1 ;;
  *) echo "Usage: $0 [--vad-only]" >&2; exit 1 ;;
esac

KWS_NAME="sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20"
KWS_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/kws-models/${KWS_NAME}.tar.bz2"

VAD_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx"
VAD_MODEL="silero_vad.onnx"

KWS_TARGET="$MODELS_DIR/sherpa-kws"
VAD_TARGET="$MODELS_DIR/silero-vad"
DOWNLOAD_DIR="$MODELS_DIR/.downloads"

fetch() {
  local url="$1"
  local dest="$2"
  if [[ -f "$dest" ]]; then
    echo "Using cached $(basename "$dest")"
    return
  fi
  echo "Downloading $url"
  local partial="$dest.partial.$$"
  if ! curl -L --fail --retry 3 --output "$partial" "$url"; then
    rm -f "$partial"
    return 1
  fi
  mv "$partial" "$dest"
}

# R1 supplies wake events; only its existing host VAD needs a model.
if [[ "${1:-}" == --vad-only ]]; then
  if [[ -s "$VAD_TARGET/$VAD_MODEL" ]]; then
    echo "Using existing $VAD_TARGET/$VAD_MODEL"
    exit 0
  fi
  mkdir -p "$VAD_TARGET" "$DOWNLOAD_DIR"
  fetch "$VAD_URL" "$DOWNLOAD_DIR/$VAD_MODEL"
  cp "$DOWNLOAD_DIR/$VAD_MODEL" "$VAD_TARGET/$VAD_MODEL"
  echo "VAD ready: $VAD_TARGET/$VAD_MODEL"
  exit 0
fi

extract() {
  local archive="$1"
  local expected_dir="$2"
  if [[ -d "$expected_dir" ]]; then
    echo "Using existing $(basename "$expected_dir")"
    return
  fi
  tar -xjf "$archive" -C "$MODELS_DIR"
}

link_or_copy() {
  local source="$1"
  local dest="$2"
  rm -f "$dest"
  cp "$source" "$dest"
}

mkdir -p "$MODELS_DIR" "$DOWNLOAD_DIR"

KEYWORDS_RAW_BACKUP=""
if [[ -f "$KWS_TARGET/keywords_raw.txt" ]]; then
  KEYWORDS_RAW_BACKUP="$(mktemp)"
  cp "$KWS_TARGET/keywords_raw.txt" "$KEYWORDS_RAW_BACKUP"
fi

KWS_ARCHIVE="$DOWNLOAD_DIR/${KWS_NAME}.tar.bz2"
VAD_DOWNLOAD="$DOWNLOAD_DIR/$VAD_MODEL"

fetch "$KWS_URL" "$KWS_ARCHIVE"
fetch "$VAD_URL" "$VAD_DOWNLOAD"

extract "$KWS_ARCHIVE" "$MODELS_DIR/$KWS_NAME"

rm -rf "$KWS_TARGET" "$VAD_TARGET"
mkdir -p "$KWS_TARGET" "$VAD_TARGET"

cp "$MODELS_DIR/$KWS_NAME/tokens.txt" "$KWS_TARGET/tokens.txt"
cp "$MODELS_DIR/$KWS_NAME/en.phone" "$KWS_TARGET/en.phone"
link_or_copy "$MODELS_DIR/$KWS_NAME/encoder-epoch-13-avg-2-chunk-8-left-64.int8.onnx" "$KWS_TARGET/encoder.onnx"
link_or_copy "$MODELS_DIR/$KWS_NAME/decoder-epoch-13-avg-2-chunk-8-left-64.onnx" "$KWS_TARGET/decoder.onnx"
link_or_copy "$MODELS_DIR/$KWS_NAME/joiner-epoch-13-avg-2-chunk-8-left-64.int8.onnx" "$KWS_TARGET/joiner.onnx"

cp "$VAD_DOWNLOAD" "$VAD_TARGET/$VAD_MODEL"

if [[ -n "$KEYWORDS_RAW_BACKUP" && -f "$KEYWORDS_RAW_BACKUP" ]]; then
  cp "$KEYWORDS_RAW_BACKUP" "$KWS_TARGET/keywords_raw.txt"
  rm -f "$KEYWORDS_RAW_BACKUP"
else
  cat > "$KWS_TARGET/keywords_raw.txt" <<'EOF'
玲珑 @玲珑
EOF
fi

validate_keywords() {
  local tokens_file="$1"
  local keywords_file="$2"
  awk '
    FNR == NR {
      known[$1] = 1
      next
    }
    /^[[:space:]]*$/ { next }
    {
      label_index = 0
      for (i = 1; i <= NF; i++) {
        if (substr($i, 1, 1) == "@") {
          label_index = i
          break
        }
      }
      if (label_index == 0) {
        printf("line %d has no @label: %s\n", FNR, $0) > "/dev/stderr"
        bad = 1
        next
      }
      for (i = 1; i < label_index; i++) {
        if (!($i in known)) {
          printf("line %d token %s is not in tokens.txt: %s\n", FNR, $i, $0) > "/dev/stderr"
          bad = 1
        }
      }
    }
    END { exit bad ? 1 : 0 }
  ' "$tokens_file" "$keywords_file"
}

if ! command -v sherpa-onnx-cli >/dev/null 2>&1; then
  cat >&2 <<EOF
error: sherpa-onnx-cli was not found, so keywords.txt cannot be generated.

Install/activate sherpa-onnx first, then rerun this script. Example:
  python3 -m venv .venv-sherpa
  source .venv-sherpa/bin/activate
  python -m pip install --upgrade pip sherpa-onnx
  ./scripts/download-sherpa-models.sh

To customize wake words, edit:
  $KWS_TARGET/keywords_raw.txt
and regenerate with:
  sherpa-onnx-cli text2token \\
    --tokens "$KWS_TARGET/tokens.txt" \\
    --tokens-type phone+ppinyin \\
    --lexicon "$KWS_TARGET/en.phone" \\
    "$KWS_TARGET/keywords_raw.txt" \\
    "$KWS_TARGET/keywords.txt"
EOF
  exit 1
fi

sherpa-onnx-cli text2token \
  --tokens "$KWS_TARGET/tokens.txt" \
  --tokens-type phone+ppinyin \
  --lexicon "$KWS_TARGET/en.phone" \
  "$KWS_TARGET/keywords_raw.txt" \
  "$KWS_TARGET/keywords.txt"

validate_keywords "$KWS_TARGET/tokens.txt" "$KWS_TARGET/keywords.txt"
echo "Generated $KWS_TARGET/keywords.txt from $KWS_TARGET/keywords_raw.txt"

cat <<EOF

Sherpa-ONNX models are ready:
  KWS:        $KWS_TARGET
  VAD:        $VAD_TARGET

These paths match alia-embedded defaults:
  models/sherpa-kws/{encoder.onnx,decoder.onnx,joiner.onnx,tokens.txt,keywords.txt}
  models/silero-vad/silero_vad.onnx
EOF
