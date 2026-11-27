#!/bin/sh
# Remove what install.sh put in place. Your data (models, learned words, user
# dictionary) is kept unless you pass --data.
set -e
DATA="${XDG_DATA_HOME:-$HOME/.local/share}/bopogram"
CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/bopogram"
sudo rm -f /usr/share/ibus/component/bopogram.xml /usr/local/bin/bopogram
sudo rm -f /usr/local/lib/bopogram/bopogram-ibus /usr/local/lib/bopogram/bopogram
sudo rmdir /usr/local/lib/bopogram 2>/dev/null || true
if [ "$1" = "--data" ]; then
    rm -rf -- "$DATA" "$CACHE"
    echo "removed $DATA and $CACHE"
else
    echo "kept $DATA (./uninstall.sh --data removes it)"
fi
ibus write-cache 2>/dev/null || true
ibus restart 2>/dev/null || true
echo "Bopogram removed. Remove it from Settings > Keyboard > Input Sources too."
