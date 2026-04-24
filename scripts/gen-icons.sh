#!/usr/bin/env bash
# Regenerate assets/iconset/, assets/hop.icns, and assets/hop.ico from assets/hop.svg.
# Commit the outputs after running this script; CI does not need rsvg-convert/iconutil.
#
# macOS:  requires rsvg-convert (brew install librsvg) + iconutil (built-in)
# Linux:  requires rsvg-convert + png2icns (apt install icnsutils) + convert (ImageMagick)
set -euo pipefail
cd "$(dirname "$0")/.."

SVG=assets/hop.svg
ISET=assets/iconset

mkdir -p "$ISET"

# Rasterise SVG → individual PNGs
for size in 16 32 64 128 256 512 1024; do
    rsvg-convert -w "$size" -h "$size" "$SVG" -o "$ISET/_${size}.png"
done

# Rename/copy to Apple iconset naming convention
cp "$ISET/_16.png"    "$ISET/icon_16x16.png"
cp "$ISET/_32.png"    "$ISET/icon_16x16@2x.png"
cp "$ISET/_32.png"    "$ISET/icon_32x32.png"
cp "$ISET/_64.png"    "$ISET/icon_32x32@2x.png"
cp "$ISET/_128.png"   "$ISET/icon_128x128.png"
cp "$ISET/_256.png"   "$ISET/icon_128x128@2x.png"
cp "$ISET/_256.png"   "$ISET/icon_256x256.png"
cp "$ISET/_512.png"   "$ISET/icon_256x256@2x.png"
cp "$ISET/_512.png"   "$ISET/icon_512x512.png"
cp "$ISET/_1024.png"  "$ISET/icon_512x512@2x.png"
rm "$ISET"/_*.png

# Also refresh hop.png (512×512 master raster used at runtime)
cp "$ISET/icon_512x512.png" assets/hop.png

# .icns — macOS icon bundle
if command -v iconutil >/dev/null 2>&1; then
    cp -r "$ISET" /tmp/hop.iconset
    iconutil -c icns /tmp/hop.iconset -o assets/hop.icns
    rm -rf /tmp/hop.iconset
elif command -v png2icns >/dev/null 2>&1; then
    png2icns assets/hop.icns "$ISET"/icon_*.png
else
    echo "WARNING: neither iconutil nor png2icns found; skipping hop.icns" >&2
fi

# .ico — Windows multi-size icon
if command -v convert >/dev/null 2>&1; then
    convert "$ISET/icon_16x16.png" \
            "$ISET/icon_32x32.png" \
            "$ISET/icon_32x32@2x.png" \
            "$ISET/icon_256x256.png" \
            assets/hop.ico
elif command -v magick >/dev/null 2>&1; then
    magick "$ISET/icon_16x16.png" \
           "$ISET/icon_32x32.png" \
           "$ISET/icon_32x32@2x.png" \
           "$ISET/icon_256x256.png" \
           assets/hop.ico
else
    echo "WARNING: ImageMagick not found; skipping hop.ico" >&2
fi

echo "Done. Commit assets/iconset/, assets/hop.icns, assets/hop.ico, assets/hop.png."
