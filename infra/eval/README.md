# memlayer AWS eval stack

One-shot [CloudFormation](template.yaml) job: boot an EC2 instance, build
memlayer from a git ref, vendor BGE-small, run the local eval suite, upload
scorecards to S3, then self-terminate.

```text
smoke → staleness → full LoCoMo (or LIMIT=N)
```

Same Make targets as a laptop (`make eval-locomo-smoke`, `eval-staleness`,
`eval-locomo`). LLM answer/judge uses **Gemini CLI** (default) or **Claude
Code**, with the API key from Secrets Manager.

## Cost (order of magnitude)

| Item | Note |
|---|---|
| EC2 `c7i.2xlarge` | ~$0.30–0.40/hour (us-east-1); terminate-on-complete is the default |
| Wall time | Full LoCoMo (~1540 QA) is **LLM-bound** — often many hours |
| Tokens | Usually dominate cost vs the instance |

Use `QueryLimit=5` (or `50`) for a cheap wiring check before a full run.

## Prerequisites

1. AWS CLI configured with permission to create CFN / EC2 / IAM / S3 / Logs.
2. A **public subnet** (or NAT) with outbound internet for apt, git, S3, Secrets Manager, and LLM APIs. Note the VPC and subnet IDs.
3. A Secrets Manager secret whose string is JSON:

```json
{ "GEMINI_API_KEY": "AIza..." }
```

For Claude instead:

```json
{ "ANTHROPIC_API_KEY": "sk-ant-..." }
```

4. LoCoMo is fetched by Make from SNAP (`locomo10.json`). Dataset terms are
   research-use; see `crates/memlayer-eval/baselines/locomo.json`.

## Deploy

```bash
# Create the LLM secret once (example: Gemini)
aws secretsmanager create-secret \
  --name memlayer/eval/gemini \
  --secret-string '{"GEMINI_API_KEY":"YOUR_KEY"}'

SECRET_ARN=$(aws secretsmanager describe-secret \
  --secret-id memlayer/eval/gemini \
  --query ARN --output text)

# Resolve default VPC + a public subnet (example helper)
VPC_ID=$(aws ec2 describe-vpcs --filters Name=isDefault,Values=true \
  --query 'Vpcs[0].VpcId' --output text)
SUBNET_ID=$(aws ec2 describe-subnets \
  --filters "Name=vpc-id,Values=${VPC_ID}" \
  --query 'Subnets[0].SubnetId' --output text)

# Cheap smoke on AWS (5 LoCoMo queries after smoke+staleness)
aws cloudformation deploy \
  --stack-name memlayer-eval-smoke \
  --template-file infra/eval/template.yaml \
  --capabilities CAPABILITY_NAMED_IAM \
  --parameter-overrides \
    VpcId="$VPC_ID" \
    SubnetId="$SUBNET_ID" \
    LlmSecretArn="$SECRET_ARN" \
    LlmProvider=gemini \
    QueryLimit=5 \
    TerminateOnComplete=true

# Full suite (omit QueryLimit)
aws cloudformation deploy \
  --stack-name memlayer-eval-full \
  --template-file infra/eval/template.yaml \
  --capabilities CAPABILITY_NAMED_IAM \
  --parameter-overrides \
    VpcId="$VPC_ID" \
    SubnetId="$SUBNET_ID" \
    LlmSecretArn="$SECRET_ARN" \
    LlmProvider=gemini \
    GitRef=main \
    TerminateOnComplete=true
```

### Useful overrides

| Parameter | Default | Purpose |
|---|---|---|
| `GitRepo` / `GitRef` | `sanudatta11/memlayer` / `main` | Source to build |
| `InstanceType` | `c7i.2xlarge` | CPU box (≥16 GB RAM recommended) |
| `VpcId` / `SubnetId` | _(required)_ | Network with outbound internet |
| `LlmProvider` | `gemini` | `gemini` or `claude` |
| `QueryLimit` | _(empty)_ | Passed as `LIMIT=` to `make eval-locomo` only |
| `KeyName` | _(empty)_ | Optional SSH key for debug |
| `TerminateOnComplete` | `true` | Set `false` to inspect the instance |

## Outputs / where to look

```bash
aws cloudformation describe-stacks \
  --stack-name memlayer-eval-smoke \
  --query 'Stacks[0].Outputs'
```

- **S3** — `s3://<ScorecardBucketName>/<stack>/<timestamp>/`
  - `locomo-smoke.json`, `staleness.json`, `staleness-baseline.json`, `locomo-full.json`
  - `manifest.json`, `memlayer-eval.log`
- **CloudWatch** — log group `/memlayer/eval/<stack>` (summary + log tail)
- **EC2 console** — instance system log while running

## Local parity

```bash
make eval-locomo-smoke
make eval-staleness
LIMIT=50 make eval-locomo   # or full without LIMIT
# Facts.db built automatically when missing (EXTRACT=0 to skip)
```

### Recommended local measure recipe (accuracy + wall-clock)

Pin OpenCode + deepseek-flash (same pair used for promotion slices):

```bash
export MEMLAYER_LLM_PROVIDER=opencode
export MEMLAYER_LLM_BIN=opencode
export MEMLAYER_LLM_MODEL=opencode-go/deepseek-v4.1-flash
# Parallel answer/judge (default 4; raise carefully for rate limits)
export MEMLAYER_EVAL_CONCURRENCY=4

LIMIT=50 make eval-locomo
# Optional: build facts.db first for fact-level hybrid
# EXTRACT=0 LIMIT=50 make eval-locomo   # ablation without facts
# EXTRACT=force LIMIT=50 make eval-locomo  # rebuild facts.db
```

`recall_at_k` / `mrr` are evidence-turn based when LoCoMo provides `evidence`
ids; `gold_substring_recall` is the older substring diagnostic. Scorecards also
report `rerank_skipped_pct` when the ambiguity gate skips LLM rerank.

For stronger answers with a cheap judge, leave the model pin on flash for
judge/rerank roles and set a capable answer model via your agent CLI’s role
mapping (memlayer answer uses the `capable`/`sonnet` role; judge uses `fast`).

Pin the same provider the AWS stack uses when several CLIs are on `PATH`:

```bash
export MEMLAYER_LLM_PROVIDER=gemini   # or claude / opencode
export MEMLAYER_LLM_BIN=gemini
```

OpenCode on the CFN AMI is out of scope until there is a supported install
recipe; keep Gemini/Claude on the stack and run OpenCode locally.

## Files

| File | Role |
|---|---|
| [template.yaml](template.yaml) | CloudFormation (S3, IAM, SG, EC2, logs) |
| [userdata.sh](userdata.sh) | Instance worker (install, build, suite, upload, terminate) |

## Out of scope (v1)

AWS Batch/ECS, prebuilt AMI/ECR, LongMemEval / BEAM jobs, public README badge publisher.
