#!/bin/sh
# Trunk post_build hook (see Trunk.toml). Adds the files the browser build serves next to the app
# but that are not part of the Rust build: the Japanese font and the licenses. Trunk runs it from
# the repository root, before it moves the staging directory to `web/dist`.
set -eu

: "${TRUNK_STAGING_DIR:?run this from Trunk (Trunk.toml hooks)}"

# IBM Plex Sans JP, which src/web.rs loads when no bundled font has a character. The site serves
# it itself, so opening the page tells no CDN. The URL is pinned to the commit of the
# `@ibm/plex-sans-jp@3.0.0` tag. The hashes are the ones in `IBM_PLEX_SANS_JP` in src/web.rs; the
# browser rejects the font if they differ, so change both together.
FONT_BASE_URL=https://raw.githubusercontent.com/IBM/plex/89cba80dad75561262e758f4b6ddd474c5119796/packages/plex-sans-jp/fonts/complete/ttf/unhinted
# Kept between builds, so only the first build (and `trunk serve`) needs the network.
FONT_CACHE_DIR=target/web-fonts

sri_sha384() {
  printf 'sha384-%s' "$(openssl dgst -sha384 -binary "$1" | openssl base64 -A)"
}

# Downloads a font into the cache unless the cached copy already has the expected hash.
fetch_font() {
  name=$1
  integrity=$2
  cached="$FONT_CACHE_DIR/$name"
  if [ -f "$cached" ] && [ "$(sri_sha384 "$cached")" = "$integrity" ]; then
    return
  fi
  mkdir -p "$FONT_CACHE_DIR"
  download="$cached.download"
  curl --fail --silent --show-error --location --retry 3 --output "$download" "$FONT_BASE_URL/$name"
  actual=$(sri_sha384 "$download")
  if [ "$actual" != "$integrity" ]; then
    rm -f "$download"
    echo "error: $FONT_BASE_URL/$name has $actual, expected $integrity" >&2
    exit 1
  fi
  mv "$download" "$cached"
}

fetch_font IBMPlexSansJP-Regular.ttf sha384-/yQZjwaTcpaqocYFEcsa32pkb1RuXBjNCLOFAOqOLZ75hLANiYYjiKWMYS7ZOX/4
fetch_font IBMPlexSansJP-SemiBold.ttf sha384-HsmSeI1bT3Exy/NcxEvE9gmfB11qUzPrmClQYUGzdkCvMIb0Hr9ct1l2VEhYzjua

mkdir -p "$TRUNK_STAGING_DIR/fonts" "$TRUNK_STAGING_DIR/licenses"
cp "$FONT_CACHE_DIR/IBMPlexSansJP-Regular.ttf" "$FONT_CACHE_DIR/IBMPlexSansJP-SemiBold.ttf" "$TRUNK_STAGING_DIR/fonts/"

# Two of the sources are named LICENSE, so each gets its own name.
cp LICENSE "$TRUNK_STAGING_DIR/licenses/csvr-LICENSE.txt"
cp THIRD_PARTY_LICENSES.html "$TRUNK_STAGING_DIR/licenses/THIRD_PARTY_LICENSES.html"
# The OFL covers IBM Plex Sans (bundled in the wasm) and IBM Plex Sans JP (in fonts/).
cp assets/fonts/ibm-plex-sans/license.txt "$TRUNK_STAGING_DIR/licenses/IBM-Plex-OFL.txt"
# The favicon is the app icon, based on a Lucide icon.
cp assets/LICENSE "$TRUNK_STAGING_DIR/licenses/icon-LICENSE.txt"

# Trunk keeps its lock file in the output directory; wrangler would otherwise publish it.
printf '.trunk-lock\n' > "$TRUNK_STAGING_DIR/.assetsignore"
