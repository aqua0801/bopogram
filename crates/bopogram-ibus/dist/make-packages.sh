#!/bin/sh
# Build the Linux release packages from the programs cross-built on Windows
# (cargo build --release -p bopogram -p bopogram-ibus --target x86_64-unknown-linux-musl)
# and the licence texts (crates/target/THIRD-PARTY-LICENSES.html, cargo about).
# Run in WSL or on Linux; needs only tar and dpkg-deb.
#
#   sh make-packages.sh OUT_DIR
#
# Writes OUT_DIR/bopogram-VERSION-linux-x86_64.tar.gz (install.sh, for any
# distribution) and OUT_DIR/bopogram_VERSION_amd64.deb (Debian, Ubuntu).
# Neither holds models: those are the model pack, installed per user.
set -e
HERE="$(cd "$(dirname "$0")" && pwd)"
CRATES="$(cd "$HERE/../.." && pwd)"
REPO="$(cd "$CRATES/.." && pwd)"
OUT="$1"
[ -n "$OUT" ] || { echo "usage: sh make-packages.sh OUT_DIR"; exit 2; }
BIN="$CRATES/target/x86_64-unknown-linux-musl/release"
LICENSES="$CRATES/target/THIRD-PARTY-LICENSES.html"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$CRATES/Cargo.toml" | head -1)"
for f in "$BIN/bopogram" "$BIN/bopogram-ibus" "$LICENSES" "$REPO/LICENSE"; do
    [ -f "$f" ] || { echo "missing: $f"; exit 1; }
done
mkdir -p "$OUT"
WORK="$(mktemp -d)"
trap 'rm -rf -- "$WORK"' EXIT

# .tar.gz
T="$WORK/bopogram-$VERSION-linux-x86_64"
mkdir -p "$T"
install -m 755 "$BIN/bopogram" "$BIN/bopogram-ibus" "$HERE/install.sh" "$HERE/uninstall.sh" "$T/"
install -m 644 "$HERE/bopogram.xml.in" "$LICENSES" "$T/"
install -m 644 "$REPO/LICENSE" "$T/LICENSE.txt"
tar -C "$WORK" -czf "$OUT/bopogram-$VERSION-linux-x86_64.tar.gz" --owner=0 --group=0 "bopogram-$VERSION-linux-x86_64"

# .deb
D="$WORK/deb"
mkdir -p "$D/DEBIAN" "$D/usr/lib/bopogram" "$D/usr/bin" "$D/usr/share/ibus/component" "$D/usr/share/doc/bopogram"
install -m 755 "$BIN/bopogram" "$BIN/bopogram-ibus" "$D/usr/lib/bopogram/"
ln -s ../lib/bopogram/bopogram "$D/usr/bin/bopogram"
sed "s#@EXEC@#/usr/lib/bopogram/bopogram-ibus --ibus#" "$HERE/bopogram.xml.in" > "$D/usr/share/ibus/component/bopogram.xml"
install -m 644 "$REPO/LICENSE" "$D/usr/share/doc/bopogram/copyright"
install -m 644 "$LICENSES" "$D/usr/share/doc/bopogram/"
SIZE="$(du -sk "$D/usr" | cut -f1)"
cat > "$D/DEBIAN/control" <<EOF
Package: bopogram
Version: $VERSION
Architecture: amd64
Maintainer: aqua0801 <79046696+aqua0801@users.noreply.github.com>
Installed-Size: $SIZE
Depends: ibus
Section: utils
Priority: optional
Homepage: https://github.com/aqua0801/bopogram
Description: context-aware Bopomofo input method for IBus
 Bopogram chooses characters from the context (word n-grams and a small
 character language model), all on this computer, without any network use.
 The models come separately as a model pack, installed per user with
 "bopogram pack install <zip>"; then run "ibus restart".
EOF
cat > "$D/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
echo "Bopogram installed. Each user: bopogram pack install <bopogram-models-*.zip>, then ibus write-cache; ibus restart,"
echo "and add it in Settings > Keyboard > Input Sources (Chinese (Taiwan) > Bopogram); close Settings first if it was open."
EOF
chmod 755 "$D/DEBIAN/postinst"
dpkg-deb --root-owner-group --build "$D" "$OUT/bopogram_${VERSION}_amd64.deb" >/dev/null
ls -l "$OUT/bopogram-$VERSION-linux-x86_64.tar.gz" "$OUT/bopogram_${VERSION}_amd64.deb"
