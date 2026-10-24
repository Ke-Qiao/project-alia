#!/usr/bin/env bash
# Software-only checks. The optional release build targets the current host.
set -euo pipefail

cd "$(dirname "$0")/.."
case "${1:-test}" in
  test|release) verification_mode="${1:-test}" ;;
  *) echo "Usage: bash scripts/verify-software.sh [test|release]" >&2; exit 2 ;;
esac

verification_container=""
cleanup() {
  local result=$?
  trap - EXIT
  if [[ -n "$verification_container" ]]; then
    docker rm -f "$verification_container" >/dev/null || result=1
  fi
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if [[ -z "${ALIA_TEST_DATABASE_URL:-}" ]]; then
  # Use an already installed image; never download or alter the developer DB.
  verification_container=$(docker run --pull=never --detach --rm \
    --label alia.task=software-verification \
    --publish 127.0.0.1::5432 \
    --env POSTGRES_USER=alia_test --env POSTGRES_PASSWORD=alia_test \
    --env POSTGRES_DB=alia_verification pgvector/pgvector:pg17)
  verification_ready=false
  for ((attempt=0; attempt<30; attempt++)); do
    if docker exec "$verification_container" pg_isready -U alia_test -d alia_verification >/dev/null 2>&1; then
      verification_ready=true
      break
    fi
    sleep 1
  done
  if [[ "$verification_ready" != true ]]; then
    echo "Disposable PostgreSQL did not become ready" >&2
    exit 1
  fi
  verification_address=$(docker port "$verification_container" 5432/tcp)
  export ALIA_TEST_DATABASE_URL="postgresql://alia_test:alia_test@${verification_address}/alia_verification"
fi

(cd examples/wasm-plugins && CARGO_TARGET_DIR="$PWD/target" cargo +1.96.0 build --release --offline --locked)
python3 scripts/check-server-boundaries.py
cargo +1.96.0 test --workspace --offline --locked --no-fail-fast
cargo +1.96.0 check -p alia-server --no-default-features --all-targets --offline --locked
cargo +1.96.0 check -p alia-embedded --no-default-features --all-targets --offline --locked
if [[ "$verification_mode" == release ]]; then
  cargo +1.96.0 build --workspace --release --offline --locked
fi
