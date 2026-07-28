#!/usr/bin/env bash
# Minimal acceptance checks for rsync-archive create + embed.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${BIN:-$ROOT/target/release/rsync-archive}"
if [[ ! -x "$BIN" ]]; then
  cargo build --release --manifest-path "$ROOT/Cargo.toml"
  BIN="$ROOT/target/release/rsync-archive"
fi

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT
cd "$WORKDIR"

mkdir -p tree/sub
echo hello > tree/a.txt
echo world > tree/sub/b.txt
echo tmp > tree/x.tmp

echo "== create dry-run =="
# trailing slash on SRC → members without root name
"$BIN" create -o out.7z -n --exclude '*.tmp' tree/ | tee dry.txt
grep -q 'a.txt' dry.txt
grep -q 'sub/b.txt' dry.txt
! grep -q 'x.tmp' dry.txt
test ! -e out.7z

echo "== create write + verify =="
"$BIN" create -o out.7z --exclude '*.tmp' --level 1 --threads 2 --verify tree/
test -f out.7z

echo "== create zstd / lz4 =="
"$BIN" create -o z.7z --method zstd --level 3 --force --verify tree/ 2>z.err
grep -q 'verify ok' z.err
grep -q 'non-solid' z.err
"$BIN" create -o l.7z --method lz4 --force --verify tree/ 2>l.err
grep -q 'verify ok' l.err
grep -q 'non-solid' l.err

echo "== create seekable-zstd + verify =="
"$BIN" create -o pack.zst --format seekable-zstd --level 1 --force --verify tree/ 2>zst.err
grep -q 'verify ok' zst.err
grep -q 'seekable-zstd' zst.err
test -f pack.zst

echo "== embed =="
"$BIN" embed -o master.7z --allow-any --force --verify out.7z
test -f master.7z

echo "== force / exists =="
if "$BIN" create -o out.7z tree/ 2>/dev/null; then
  echo "expected overwrite error" >&2
  exit 1
fi
"$BIN" create -o out.7z --force --level 1 tree/

echo "v1 acceptance OK"
