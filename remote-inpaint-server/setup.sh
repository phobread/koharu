#!/usr/bin/env bash
# One-shot setup + launch for a RunPod pod. Run from this directory:
#   bash setup.sh
# Environment (set in the RunPod template or export before running):
#   API_KEY   token the koharu client must present (pick any random string)
#   HF_TOKEN  Hugging Face token with FLUX.1-Fill-dev access (gated repo)
#   MODEL     flux-fill (default) | opencv (instant CPU smoke test)
set -euo pipefail
cd "$(dirname "$0")"

echo "== installing dependencies =="
pip install --quiet -r requirements.txt

if [ "${MODEL:-flux-fill}" = "flux-fill" ] && [ -z "${HF_TOKEN:-}" ]; then
    echo "WARNING: HF_TOKEN is not set — the gated FLUX.1-Fill-dev download will fail." >&2
    echo "Accept the license at https://huggingface.co/black-forest-labs/FLUX.1-Fill-dev" >&2
    echo "and set HF_TOKEN to a read token from https://huggingface.co/settings/tokens" >&2
fi

echo "== starting server on port ${PORT:-8000} =="
echo "   (first flux-fill start downloads ~30GB of weights; watch this log)"
exec uvicorn server:app --host 0.0.0.0 --port "${PORT:-8000}"
