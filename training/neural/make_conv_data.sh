#!/usr/bin/env bash
# Conversational re-ranking data from FineWeb2 file 5 (blogs and forums), which
# neither the n-gram models nor the character model were trained on.
# See prototype/docs/charlm-rerank-training.md.
#
# Usage (WSL, from training/, after pipeline/fineweb_extract.py ... --files 5 [6-8]):
#   [RERANK_OUT=rerank_conv RERANK_N=300000] bash neural/make_conv_data.sh
# More extracted files are taken in as they appear; a different RERANK_OUT keeps earlier samples.
# Every step skips finished shards, so the script can be run again after an interruption.
set -euo pipefail

D=${BOPOGRAM_DATA:-$HOME/bopogram-data}
CFG=$D/dist/bopogram-runtime/config.json
OUT=${RERANK_OUT:-rerank_conv}
N=${RERANK_N:-300000}

echo "== conversational pages $(date '+%T')"
uv run python pipeline/fineweb_conv.py "$D/extracted/fineweb2_extra" "$D/extracted/fineweb2_conv"
echo "== clean $(date '+%T')"
uv run python pipeline/clean.py "$D/extracted/fineweb2_conv" "$D/clean_conv/fineweb2_conv"
echo "== mix (China-usage filter) $(date '+%T')"
uv run python pipeline/mix.py "$D/clean_conv" "$D/corpus_conv" --replan
echo "== segment $(date '+%T')"
uv run python pipeline/segment.py run "$D/corpus_conv" "$D/dict/dict.tsv" "$D/segmented/counts.tsv" "$D/segmented_conv"
echo "== readings $(date '+%T')"
mkdir -p "$D/neural/data_conv"
cp -n "$D/neural/data/vocab.json" "$D/neural/data_conv/vocab.json"
uv run python neural/make_data.py run "$D/segmented_conv" "$D/dict/dict.tsv" "$D/segmented/counts.tsv" "$D/neural/data_conv"
echo "== re-ranking examples $(date '+%T')"
uv run --project ../prototype python neural/make_rerank_data.py "$CFG" "$D/neural/data_conv" "$D/neural/$OUT" \
  --sources fineweb2_conv=$N --workers 4
uv run --project ../prototype python neural/make_rerank_data.py "$CFG" "$D/neural/data_conv" "$D/neural/rerank_conv_val" \
  --sources fineweb2_conv=2000 --heldout --workers 4
echo "== done $(date '+%T')"
