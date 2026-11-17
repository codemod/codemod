#!/usr/bin/env bash
# run.sh <arm: alone|assistant> <n>   one fresh codeimage checkout, one headless agent run, then grading
set -uo pipefail
ARM=$1; N=$2
AVS=/home/user/scratch/avs; OUT=/mnt/project-files/agent-vs-sma/runs/$ARM-$N
DIR=/home/user/scratch/runs/$ARM-$N; AHOME=/home/user/scratch/runs/home-$ARM-$N
MODEL=${AGENT_MODEL:-sonnet}; BUDGET=${AGENT_BUDGET:-24}; LIMIT=${AGENT_TIMEOUT:-9000}
rm -rf "$DIR" "$AHOME"; mkdir -p "$OUT" "$AHOME" "$(dirname "$DIR")"
git clone --quiet "$AVS/codeimage-base" "$DIR"
(cd "$DIR" && CI=1 HUSKY=0 pnpm install --frozen-lockfile >/dev/null 2>&1 && pnpm libs:build >/dev/null 2>&1) || { echo "setup failed" > "$OUT/error"; exit 1; }
mkdir -p "$AHOME/docs" && cp /mnt/project-files/agent-vs-sma/MIGRATION-solid-2.0.0.md "$AHOME/docs/MIGRATION.md"
PROMPT=$(sed "s#DOCS_DIR#$AHOME/docs#" /mnt/project-files/agent-vs-sma/prompts/base.md)
TOOLS=("Bash(pnpm:*)")
P=/opt/node22/bin:/usr/local/bin:/usr/bin:/bin
if [ "$ARM" = assistant ]; then PROMPT="$PROMPT$(cat /mnt/project-files/agent-vs-sma/prompts/assistant.md)"; P="$AVS/bin:$P"; TOOLS+=("Bash(solid-migration-assistant:*)"); fi
START=$(date +%s)
cd "$DIR" && env -i PATH="$P" HOME="$AHOME" ANTHROPIC_BASE_URL="$ANTHROPIC_BASE_URL" \
  CLAUDE_SESSION_INGRESS_TOKEN_FILE="$CLAUDE_SESSION_INGRESS_TOKEN_FILE" \
  HTTPS_PROXY="$HTTPS_PROXY" https_proxy="$https_proxy" NO_PROXY="$NO_PROXY" no_proxy="$no_proxy" \
  NODE_EXTRA_CA_CERTS="$NODE_EXTRA_CA_CERTS" SSL_CERT_FILE="$SSL_CERT_FILE" REQUESTS_CA_BUNDLE="$REQUESTS_CA_BUNDLE" \
  NPM_CONFIG_USERCONFIG="$NPM_CONFIG_USERCONFIG" npm_config_https_proxy="$npm_config_https_proxy" npm_config_noproxy="$npm_config_noproxy" \
  npm_config_store_dir=/root/.local/share/pnpm/store/v10 CI=1 HUSKY=0 \
  timeout "$LIMIT" claude -p "$PROMPT" --model "$MODEL" --max-budget-usd "$BUDGET" \
    --output-format stream-json --verbose --permission-mode acceptEdits --permission-prompts none --add-dir "$AHOME/docs" --allowedTools "${TOOLS[@]}" \
    --strict-mcp-config --setting-sources "" --no-session-persistence \
  > "$OUT/transcript.jsonl" 2> "$OUT/stderr.log"
echo $? > "$OUT/agent-exit"; END=$(date +%s); echo $((END-START)) > "$OUT/wall-seconds"
python3 /mnt/project-files/agent-vs-sma/grade.py "$DIR" --baseline /mnt/project-files/agent-vs-sma/baseline-solid1.json > "$OUT/grade.json" 2> "$OUT/grade.err"
(cd "$DIR" && git add -A -N . && git diff -- . ':!pnpm-lock.yaml' > "$OUT/changes.diff"; git reset -q)
echo done > "$OUT/status"
