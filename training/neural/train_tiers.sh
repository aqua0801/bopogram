#!/usr/bin/env bash
# Train charlm tiers (prototype/docs/charlm-tiers.md), one after another on the GPU.
#
# Usage (WSL, from training/):  bash neural/train_tiers.sh TIER...
#   TIER: lite | std2x | advanced | max
#
# Every tier uses the recipe of the current model (runs/charlm_eos, then
# runs/charlm_fw): stage A on the general corpora, stage B adds FineWeb2,
# starting from stage A's weights. Run again after an interruption: charlm.py
# resumes from last.pt, and a finished stage stops at once.
set -euo pipefail

D=${BOPOGRAM_DATA:-$HOME/bopogram-data}
DATA=$D/neural/data
RUNS=$D/neural/runs
LOGS=$D/logs
mkdir -p "$LOGS"

W_A="twllm=0.20,ly_gazette=0.22,zhwiki=0.30"
W_B="fineweb2=0.40,ey=0.024,ey_news=0.036,judicial=0.048,ly_gazette=0.144,twllm=0.132,zhwiki=0.192,zhwikinews=0.024"

train() {  # name size_args steps_a steps_b
  local name=$1 size=$2 sa=$3 sb=$4
  echo "== $name stage A ($sa steps) $(date '+%F %T')"
  uv run python neural/charlm.py "$DATA" "$RUNS/${name}_a" $size --steps "$sa" --lr 4e-4 --warmup 3000 \
    --eval-every 10000 --weights "$W_A" >> "$LOGS/${name}_a.log" 2>&1
  echo "== $name stage B ($sb steps) $(date '+%F %T')"
  uv run python neural/charlm.py "$DATA" "$RUNS/${name}_b" $size --init "$RUNS/${name}_a/last.pt" \
    --steps "$sb" --lr 2e-4 --warmup 1000 --eval-every 5000 --weights "$W_B" >> "$LOGS/${name}_b.log" 2>&1
  echo "== $name done $(date '+%F %T')"
}

for tier in "$@"; do
  case $tier in
    lite)     train charlm_lite     "--d-model 256 --layers 4 --heads 4"   80000  30000 ;;
    std2x)    train charlm_std2x    "--d-model 384 --layers 6 --heads 6"   160000 60000 ;;
    advanced) train charlm_advanced "--d-model 512 --layers 8 --heads 8"   80000  30000 ;;
    max)      train charlm_max      "--d-model 768 --layers 12 --heads 12" 80000  30000 ;;
    *) echo "unknown tier: $tier" >&2; exit 2 ;;
  esac
done
