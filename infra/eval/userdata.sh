#!/usr/bin/env bash
# memlayer AWS eval one-shot job.
# Invoked from CloudFormation UserData with env vars set (see template.yaml).
set -euo pipefail

LOG_FILE="${LOG_FILE:-/var/log/memlayer-eval.log}"
mkdir -p "$(dirname "$LOG_FILE")"
exec > >(tee -a "$LOG_FILE") 2>&1

echo "=== memlayer eval userdata start $(date -u +%Y-%m-%dT%H:%M:%SZ) ==="

: "${SCORECARD_BUCKET:?SCORECARD_BUCKET required}"
: "${STACK_NAME:?STACK_NAME required}"
: "${AWS_REGION:?AWS_REGION required}"
: "${GIT_REPO:?GIT_REPO required}"
: "${GIT_REF:?GIT_REF required}"
: "${LLM_PROVIDER:?LLM_PROVIDER required}"
: "${LLM_SECRET_ARN:?LLM_SECRET_ARN required}"
: "${LOG_GROUP:?LOG_GROUP required}"
: "${INSTANCE_TYPE:=unknown}"
: "${TERMINATE_ON_COMPLETE:=true}"
: "${QUERY_LIMIT:=}"
: "${GEMINI_CLI_VERSION:=0.59.0}"

export DEBIAN_FRONTEND=noninteractive
START_EPOCH=$(date +%s)
RUN_TS=$(date -u +%Y%m%dT%H%M%SZ)
WORK_DIR=/opt/memlayer
SCORE_PREFIX="${STACK_NAME}/${RUN_TS}"
SUITE_STATUS=failed
GIT_SHA=unknown

on_exit() {
  local code=$?
  set +e
  END_EPOCH=$(date +%s)
  DURATION_SECS=$((END_EPOCH - START_EPOCH))

  if [[ -d "${WORK_DIR}/eval" ]]; then
    aws s3 sync "${WORK_DIR}/eval/" "s3://${SCORECARD_BUCKET}/${SCORE_PREFIX}/" \
      --region "${AWS_REGION}" || true
  fi
  aws s3 cp "${LOG_FILE}" "s3://${SCORECARD_BUCKET}/${SCORE_PREFIX}/memlayer-eval.log" \
    --region "${AWS_REGION}" || true

  cat > /tmp/memlayer-eval-manifest.json <<EOF
{
  "stack_name": "${STACK_NAME}",
  "git_repo": "${GIT_REPO}",
  "git_ref": "${GIT_REF}",
  "git_sha": "${GIT_SHA}",
  "instance_type": "${INSTANCE_TYPE}",
  "llm_provider": "${LLM_PROVIDER}",
  "query_limit": "${QUERY_LIMIT}",
  "suite_status": "${SUITE_STATUS}",
  "started_at": "$(date -u -d "@${START_EPOCH}" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -r "${START_EPOCH}" +%Y-%m-%dT%H:%M:%SZ)",
  "finished_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "duration_secs": ${DURATION_SECS},
  "scorecard_prefix": "s3://${SCORECARD_BUCKET}/${SCORE_PREFIX}/",
  "exit_code": ${code}
}
EOF
  aws s3 cp /tmp/memlayer-eval-manifest.json \
    "s3://${SCORECARD_BUCKET}/${SCORE_PREFIX}/manifest.json" \
    --region "${AWS_REGION}" || true

  # Best-effort CloudWatch: one log stream with a summary + tail of the run log.
  STREAM="run-${RUN_TS}"
  aws logs create-log-stream \
    --log-group-name "${LOG_GROUP}" \
    --log-stream-name "${STREAM}" \
    --region "${AWS_REGION}" 2>/dev/null || true
  SUMMARY="suite_status=${SUITE_STATUS} exit=${code} duration_secs=${DURATION_SECS} prefix=s3://${SCORECARD_BUCKET}/${SCORE_PREFIX}/"
  TS_MS=$(($(date +%s) * 1000))
  CW_JSON=/tmp/memlayer-cw-events.json
  TAIL_FILE=/tmp/memlayer-eval-tail.txt
  tail -c 200000 "${LOG_FILE}" 2>/dev/null | tr -d '\0' > "${TAIL_FILE}" || true
  python3 -c '
import json, sys
group, stream, ts, summary, tail_path, out_path = sys.argv[1:7]
tail = open(tail_path, "r", errors="replace").read()
msg = (summary + "\n--- log tail ---\n" + tail)[:250000]
json.dump({
    "logGroupName": group,
    "logStreamName": stream,
    "logEvents": [{"timestamp": int(ts), "message": msg}],
}, open(out_path, "w"))
' "${LOG_GROUP}" "${STREAM}" "${TS_MS}" "${SUMMARY}" "${TAIL_FILE}" "${CW_JSON}" 2>/dev/null \
    && aws logs put-log-events --region "${AWS_REGION}" --cli-input-json "file://${CW_JSON}" \
    || true

  if [[ "${TERMINATE_ON_COMPLETE}" == "true" ]]; then
    TOKEN=$(curl -sf -X PUT "http://169.254.169.254/latest/api/token" \
      -H "X-aws-ec2-metadata-token-ttl-seconds: 21600" || true)
    if [[ -n "${TOKEN}" ]]; then
      IID=$(curl -sf -H "X-aws-ec2-metadata-token: ${TOKEN}" \
        http://169.254.169.254/latest/meta-data/instance-id || true)
    else
      IID=$(curl -sf http://169.254.169.254/latest/meta-data/instance-id || true)
    fi
    if [[ -n "${IID}" ]]; then
      echo "Terminating instance ${IID} (TerminateOnComplete=true)"
      aws ec2 terminate-instances --instance-ids "${IID}" --region "${AWS_REGION}" || true
    fi
  fi
  echo "=== memlayer eval userdata end $(date -u +%Y-%m-%dT%H:%M:%SZ) exit=${code} ==="
}
trap on_exit EXIT

# ── System packages ──────────────────────────────────────────────────────────
apt-get update -y
apt-get install -y \
  build-essential pkg-config libssl-dev protobuf-compiler \
  curl git jq ca-certificates python3 unzip

# AWS CLI v2 (AL/Ubuntu images may ship v1 or none)
if ! command -v aws >/dev/null 2>&1 || ! aws --version 2>&1 | grep -q 'aws-cli/2'; then
  curl -fsSL "https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip" -o /tmp/awscliv2.zip
  unzip -q /tmp/awscliv2.zip -d /tmp
  /tmp/aws/install --update
  rm -rf /tmp/aws /tmp/awscliv2.zip
fi

# Node 22 for Gemini / Claude CLIs
if ! command -v node >/dev/null 2>&1 || [[ "$(node -v | sed 's/v//' | cut -d. -f1)" -lt 18 ]]; then
  curl -fsSL https://deb.nodesource.com/setup_22.x | bash -
  apt-get install -y nodejs
fi

# Rust stable
if ! command -v rustc >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
fi
# shellcheck disable=SC1091
source "${HOME}/.cargo/env"
export PATH="${HOME}/.cargo/bin:${PATH}"

# ── LLM CLI + secret ─────────────────────────────────────────────────────────
SECRET_JSON=$(aws secretsmanager get-secret-value \
  --secret-id "${LLM_SECRET_ARN}" \
  --region "${AWS_REGION}" \
  --query SecretString \
  --output text)

case "${LLM_PROVIDER}" in
  gemini)
    npm install -g "@google/gemini-cli@${GEMINI_CLI_VERSION}"
    export GEMINI_API_KEY
    GEMINI_API_KEY=$(echo "${SECRET_JSON}" | jq -r '.GEMINI_API_KEY // empty')
    if [[ -z "${GEMINI_API_KEY}" ]]; then
      echo "ERROR: secret must contain GEMINI_API_KEY for LlmProvider=gemini" >&2
      exit 1
    fi
    export MEMLAYER_LLM_PROVIDER=gemini
    export MEMLAYER_LLM_BIN=gemini
    ;;
  claude)
    npm install -g @anthropic-ai/claude-code
    export ANTHROPIC_API_KEY
    ANTHROPIC_API_KEY=$(echo "${SECRET_JSON}" | jq -r '.ANTHROPIC_API_KEY // empty')
    if [[ -z "${ANTHROPIC_API_KEY}" ]]; then
      echo "ERROR: secret must contain ANTHROPIC_API_KEY for LlmProvider=claude" >&2
      exit 1
    fi
    export MEMLAYER_LLM_PROVIDER=claude
    export MEMLAYER_LLM_BIN=claude
    ;;
  *)
    echo "ERROR: unsupported LLM_PROVIDER=${LLM_PROVIDER} (use gemini|claude)" >&2
    exit 1
    ;;
esac

# ── Clone + build ────────────────────────────────────────────────────────────
rm -rf "${WORK_DIR}"
mkdir -p "${WORK_DIR}"
if git ls-remote --heads "${GIT_REPO}" "${GIT_REF}" | grep -q .; then
  git clone --depth 1 --branch "${GIT_REF}" "${GIT_REPO}" "${WORK_DIR}"
elif git ls-remote --tags "${GIT_REPO}" "${GIT_REF}" | grep -q .; then
  git clone --depth 1 --branch "${GIT_REF}" "${GIT_REPO}" "${WORK_DIR}"
else
  git clone "${GIT_REPO}" "${WORK_DIR}"
  git -C "${WORK_DIR}" checkout "${GIT_REF}"
fi
GIT_SHA=$(git -C "${WORK_DIR}" rev-parse HEAD)
echo "Cloned ${GIT_REPO} @ ${GIT_REF} (sha=${GIT_SHA})"

cd "${WORK_DIR}"
cargo build --release -p memlayer-cli

# ── Eval suite (fail-fast; always upload via trap) ───────────────────────────
echo ">>> make eval-locomo-smoke"
make eval-locomo-smoke

echo ">>> make eval-staleness"
make eval-staleness

echo ">>> make eval-locomo"
if [[ -n "${QUERY_LIMIT}" ]]; then
  echo "QueryLimit=${QUERY_LIMIT} → LIMIT=${QUERY_LIMIT} for eval-locomo only"
  LIMIT="${QUERY_LIMIT}" make eval-locomo
else
  make eval-locomo
fi

SUITE_STATUS=success
echo "Suite completed successfully"
exit 0
