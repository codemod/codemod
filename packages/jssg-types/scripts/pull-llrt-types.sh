#!/usr/bin/env bash
set -euo pipefail

# Pulls LLRT type definitions from https://github.com/awslabs/llrt/tree/main/types
# into packages/jssg-types/src/llrt/

REPO="awslabs/llrt"
BRANCH="main"
REMOTE_DIR="types"
BASE_URL="https://raw.githubusercontent.com/${REPO}/${BRANCH}/${REMOTE_DIR}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DEST_DIR="${SCRIPT_DIR}/../src/llrt"

# Files to pull (parsed from the LLRT index.d.ts)
FILES=(
  abort.d.ts
  assert.d.ts
  async_hooks.d.ts
  buffer.d.ts
  child_process.d.ts
  console.d.ts
  crypto.d.ts
  dgram.d.ts
  dns.d.ts
  dom-events.d.ts
  events.d.ts
  exceptions.d.ts
  fetch.d.ts
  fs.d.ts
  fs/promises.d.ts
  globals.d.ts
  https.d.ts
  module.d.ts
  navigator.d.ts
  net.d.ts
  os.d.ts
  path.d.ts
  perf_hooks.d.ts
  process.d.ts
  stream.d.ts
  stream/web.d.ts
  string_decoder.d.ts
  timers.d.ts
  timezone.d.ts
  tty.d.ts
  url.d.ts
  util.d.ts
  zlib.d.ts
)

echo "Pulling LLRT types from ${REPO}@${BRANCH}..."

# Clean and recreate destination
rm -rf "${DEST_DIR}"
mkdir -p "${DEST_DIR}/fs" "${DEST_DIR}/stream"

for file in "${FILES[@]}"; do
  echo "  ${file}"
  curl -sf "${BASE_URL}/${file}" -o "${DEST_DIR}/${file}"
done

# Write the llrt index that references all the files
INDEX="${DEST_DIR}/index.d.ts"
: > "${INDEX}"
for file in "${FILES[@]}"; do
  echo "/// <reference path=\"./${file}\" />" >> "${INDEX}"
done

echo "Done. Files written to src/llrt/"
