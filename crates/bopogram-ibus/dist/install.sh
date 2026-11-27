#!/bin/sh
# Install Bopogram for IBus, from the folder this script is in.
#
#   your data folder  ~/.local/share/bopogram/   config, dictionary, models (no sudo)
#   the program       /usr/local/lib/bopogram/bopogram-ibus                (sudo)
#   IBus's component  /usr/share/ibus/component/bopogram.xml               (sudo)
#
# Nothing else is changed. ./uninstall.sh takes it away again.
set -e
cd "$(dirname "$0")"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}/bopogram"
LIB=/usr/local/lib/bopogram
COMPONENT=/usr/share/ibus/component/bopogram.xml

for f in bopogram-ibus config.json dict.tsv reading_prior.tsv models charlm bopogram.xml.in; do
    [ -e "$f" ] || { echo "missing: $f (run this from the unpacked kit)"; exit 1; }
done
command -v ibus >/dev/null || { echo "IBus is not installed (Ubuntu: sudo apt install ibus)"; exit 1; }

echo "== data -> $DATA"
mkdir -p "$DATA/userdata"
cp config.json dict.tsv reading_prior.tsv DO_NOT_REDISTRIBUTE.txt "$DATA/"
rm -rf "$DATA/models" "$DATA/charlm"
cp -r models charlm "$DATA/"

echo "== program -> $LIB (sudo)"
sudo install -d "$LIB"
sudo install -m 755 bopogram-ibus "$LIB/bopogram-ibus"

echo "== IBus component -> $COMPONENT (sudo)"
sed "s#@EXEC@#$LIB/bopogram-ibus --ibus#" bopogram.xml.in | sudo tee "$COMPONENT" >/dev/null

echo "== check (types three sentences without IBus)"
BOPOGRAM_DATA="$DATA" "$LIB/bopogram-ibus" --check

echo "== restart IBus"
ibus write-cache 2>/dev/null || true
ibus restart 2>/dev/null || ibus-daemon -drx

cat <<'EOF'

Done. To use it:
  Settings > Keyboard > Input Sources > + > Chinese (Taiwan) > Bopogram
  (or try it at once: ibus engine bopogram)
Switch input sources with Super+Space. Shift alone switches Chinese/English.
Log (never contains typed text): ~/.cache/bopogram/ibus.log
EOF
