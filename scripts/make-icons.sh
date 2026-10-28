#!/bin/sh
# Build every icon from the logo artwork (needs ImageMagick 7).
#   scripts/make-icons.sh LOGO.jpg [LEFT TOP SIZE RADIUS]
# The artwork is a rounded square on a dark backdrop; LEFT TOP SIZE RADIUS (pixels in the
# artwork) say where the square sits, so it can be cut out onto transparency. The defaults
# fit the 2048x2048 logo from 2026-10-28.
set -eu
SRC=${1:?usage: scripts/make-icons.sh LOGO.jpg [LEFT TOP SIZE RADIUS]}
L=${2:-377} T=${3:-377} S=${4:-1296} R=${5:-258}
cd "$(dirname "$0")/.."
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
read -r W H <<EOF
$(magick identify -format "%w %h" "$SRC")
EOF
magick -size "${W}x${H}" xc:black -fill white \
  -draw "roundrectangle $L,$T $((L + S - 1)),$((T + S - 1)) $R,$R" "$tmp/mask.png"
magick "$SRC" "$tmp/mask.png" -alpha off -compose CopyOpacity -composite \
  -crop "${S}x${S}+$L+$T" +repage "$tmp/cut.png"
# The square fills 92% of the canvas, leaving the margin hicolor icons usually have.
magick "$tmp/cut.png" -filter Lanczos -resize 944x944 -background none -gravity center \
  -extent 1024x1024 -strip assets/acreage-1024.png
for s in 16 22 24 32 48 64 128 256 512; do
  mkdir -p "assets/icons/${s}x${s}"
  magick assets/acreage-1024.png -filter Lanczos -resize "${s}x${s}" -strip "assets/icons/${s}x${s}/acreage.png"
done
# Website: favicons, the header mark and the hero mark (2x for sharp screens).
magick assets/acreage-1024.png -filter Lanczos -resize 32x32 -strip site/favicon-32.png
magick assets/acreage-1024.png -filter Lanczos -resize 180x180 -strip site/apple-touch-icon.png
magick assets/acreage-1024.png -filter Lanczos -resize 64x64 -strip site/img/logo-64.png
magick assets/acreage-1024.png -filter Lanczos -resize 192x192 -strip site/img/logo-192.png
echo "Done. Check with: git status"
