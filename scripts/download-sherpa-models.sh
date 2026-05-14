#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODELS_DIR="${MODELS_DIR:-$ROOT_DIR/config/models}"

KWS_NAME="sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20"
KWS_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/kws-models/${KWS_NAME}.tar.bz2"

SENSE_NAME="sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09"
SENSE_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/${SENSE_NAME}.tar.bz2"

KWS_TARGET="$MODELS_DIR/sherpa-kws"
SENSE_TARGET="$MODELS_DIR/sense-voice"
DOWNLOAD_DIR="$MODELS_DIR/.downloads"

fetch() {
  local url="$1"
  local dest="$2"
  if [[ -f "$dest" ]]; then
    echo "Using cached $(basename "$dest")"
    return
  fi
  echo "Downloading $url"
  curl -L --fail --retry 3 --output "$dest" "$url"
}

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
  if ln -s "$(basename "$source")" "$dest" 2>/dev/null; then
    return
  fi
  cp "$source" "$dest"
}

mkdir -p "$MODELS_DIR" "$DOWNLOAD_DIR"

KWS_ARCHIVE="$DOWNLOAD_DIR/${KWS_NAME}.tar.bz2"
SENSE_ARCHIVE="$DOWNLOAD_DIR/${SENSE_NAME}.tar.bz2"

fetch "$KWS_URL" "$KWS_ARCHIVE"
fetch "$SENSE_URL" "$SENSE_ARCHIVE"

extract "$KWS_ARCHIVE" "$MODELS_DIR/$KWS_NAME"
extract "$SENSE_ARCHIVE" "$MODELS_DIR/$SENSE_NAME"

rm -rf "$KWS_TARGET" "$SENSE_TARGET"
mkdir -p "$KWS_TARGET" "$SENSE_TARGET"

cp "$MODELS_DIR/$KWS_NAME/tokens.txt" "$KWS_TARGET/tokens.txt"
cp "$MODELS_DIR/$KWS_NAME/en.phone" "$KWS_TARGET/en.phone"
link_or_copy "$MODELS_DIR/$KWS_NAME/encoder-epoch-13-avg-2-chunk-8-left-64.int8.onnx" "$KWS_TARGET/encoder.onnx"
link_or_copy "$MODELS_DIR/$KWS_NAME/decoder-epoch-13-avg-2-chunk-8-left-64.onnx" "$KWS_TARGET/decoder.onnx"
link_or_copy "$MODELS_DIR/$KWS_NAME/joiner-epoch-13-avg-2-chunk-8-left-64.int8.onnx" "$KWS_TARGET/joiner.onnx"

cp "$MODELS_DIR/$SENSE_NAME/model.int8.onnx" "$SENSE_TARGET/model.int8.onnx"
cp "$MODELS_DIR/$SENSE_NAME/tokens.txt" "$SENSE_TARGET/tokens.txt"

cat > "$KWS_TARGET/keywords_raw.txt" <<'EOF'
HEY ALIA @HEY_ALIA
你好阿利亚 @你好阿利亚
EOF

if command -v sherpa-onnx-cli >/dev/null 2>&1; then
  sherpa-onnx-cli text2token \
    --tokens "$KWS_TARGET/tokens.txt" \
    --tokens-type phone+ppinyin \
    --lexicon "$KWS_TARGET/en.phone" \
    "$KWS_TARGET/keywords_raw.txt" \
    "$KWS_TARGET/keywords.txt"
  echo "Generated $KWS_TARGET/keywords.txt"
else
  cp "$MODELS_DIR/$KWS_NAME/test_wavs/keywords.txt" "$KWS_TARGET/keywords.txt"
  cat >&2 <<EOF
warning: sherpa-onnx-cli was not found, so the sample upstream keywords.txt was copied.
Install sherpa-onnx-cli and regenerate project keywords with:

  sherpa-onnx-cli text2token \\
    --tokens "$KWS_TARGET/tokens.txt" \\
    --tokens-type phone+ppinyin \\
    --lexicon "$KWS_TARGET/en.phone" \\
    "$KWS_TARGET/keywords_raw.txt" \\
    "$KWS_TARGET/keywords.txt"
EOF
fi

cat <<EOF

Sherpa-ONNX models are ready:
  KWS:        $KWS_TARGET
  SenseVoice: $SENSE_TARGET

These paths match alia-embedded defaults:
  models/sherpa-kws/{encoder.onnx,decoder.onnx,joiner.onnx,tokens.txt,keywords.txt}
  models/sense-voice/{model.int8.onnx,tokens.txt}
EOF
