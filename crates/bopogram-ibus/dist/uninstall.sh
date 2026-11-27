#!/bin/sh
# Remove what install.sh put in place. Your data folder (learned words, user
# dictionary) is kept unless you pass --data.
set -e
DATA="${XDG_DATA_HOME:-$HOME/.local/share}/bopogram"
sudo rm -f /usr/share/ibus/component/bopogram.xml
sudo rm -rf /usr/local/lib/bopogram
if [ "$1" = "--data" ]; then
    rm -rf "$DATA"
    echo "removed $DATA"
else
    echo "kept $DATA (./uninstall.sh --data removes it)"
fi
ibus write-cache 2>/dev/null || true
ibus restart 2>/dev/null || true
echo "Bopogram removed. Remove it from Settings > Keyboard > Input Sources too."
