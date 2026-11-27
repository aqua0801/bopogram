#!/bin/sh
# Install Bopogram for IBus from this folder (the .tar.gz release).
#
#   ./install.sh [bopogram-models-VERSION.zip]
#
#   the programs      /usr/local/lib/bopogram/   bopogram-ibus, bopogram   (sudo)
#                     /usr/local/bin/bopogram    the command line         (sudo)
#   IBus's component  /usr/share/ibus/component/bopogram.xml           (sudo)
#   a model pack      ~/.local/share/bopogram/   (bopogram pack install; no sudo)
#
# Nothing else is changed; ./uninstall.sh takes it away again. Model packs
# are files you download; nothing here uses the network.
set -e
cd "$(dirname "$0")"
LIB=/usr/local/lib/bopogram
COMPONENT=/usr/share/ibus/component/bopogram.xml
PACK="$1"

for f in bopogram-ibus bopogram bopogram.xml.in; do
    [ -e "$f" ] || { echo "missing: $f (run this from the unpacked folder)"; exit 1; }
done
[ -z "$PACK" ] || [ -f "$PACK" ] || { echo "no such file: $PACK"; exit 1; }
command -v ibus >/dev/null || { echo "IBus is not installed (Ubuntu: sudo apt install ibus)"; exit 1; }

echo "== programs -> $LIB (sudo)"
sudo install -d "$LIB"
sudo install -m 755 bopogram-ibus bopogram "$LIB/"
sudo ln -sf "$LIB/bopogram" /usr/local/bin/bopogram

echo "== IBus component -> $COMPONENT (sudo)"
sed "s#@EXEC@#$LIB/bopogram-ibus --ibus#" bopogram.xml.in | sudo tee "$COMPONENT" >/dev/null

if [ -n "$PACK" ]; then
    echo "== model pack (checking, building the dictionary, self-test)"
    "$LIB/bopogram" pack install "$PACK" 2>&1 | grep -v '^ *[0-9]*% '
fi
"$LIB/bopogram" pack status | grep -q '"current":"' || {
    echo
    echo "No model pack is installed yet. Download one and run:"
    echo "  bopogram pack install bopogram-models-VERSION.zip && ibus restart"
}

echo "== restart IBus"
ibus write-cache 2>/dev/null || true
ibus restart 2>/dev/null || ibus-daemon -drx

cat <<'EOF'

Done. To use it:
  Settings > Keyboard > Input Sources > + > Chinese (Taiwan) > Bopogram
Switch input sources with Super+Space. Shift alone switches Chinese/English.
Model packs later: bopogram pack install <zip>, then ibus restart
  (bopogram pack rollback goes back; bopogram pack tier lite|standard|advanced|off).
Log (never contains typed text): ~/.cache/bopogram/ibus.log
EOF
