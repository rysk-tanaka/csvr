#!/bin/sh
# Trunk post_build hook (see Trunk.toml). Adds the files the browser build serves next to the app
# but that are not part of the Rust build: the Japanese font, the licenses, and `_headers` for
# Cloudflare Workers. Trunk runs it from the repository root, after it has written the final
# index.html to the staging directory and before it moves that directory to `web/dist`.
set -eu

: "${TRUNK_STAGING_DIR:?run this from Trunk (Trunk.toml hooks)}"

# IBM Plex Sans JP, which src/web.rs loads when no bundled font has a character. The site serves
# it itself, so opening the page tells no CDN. The URL is pinned to the commit of the
# `@ibm/plex-sans-jp@3.0.0` tag.
FONT_BASE_URL=https://raw.githubusercontent.com/IBM/plex/89cba80dad75561262e758f4b6ddd474c5119796/packages/plex-sans-jp/fonts/complete/ttf/unhinted
# Kept between builds, so only the first `trunk build` or `trunk serve` needs the network.
FONT_CACHE_DIR=target/web-fonts

# A release build must not go out without the font. A debug build (`trunk serve`) works without
# it, as before the font was served from the site: text the bundled font lacks stays tofu.
if [ "${TRUNK_PROFILE:-}" = release ]; then
  is_release=true
else
  is_release=false
fi

sri_sha384() {
  printf 'sha384-%s' "$(openssl dgst -sha384 -binary "$1" | openssl base64 -A)"
}

# The file names and hashes come from `IBM_PLEX_SANS_JP` in src/web.rs, so the files here are the
# ones the app asks for and the browser accepts. Each `url: "fonts/<name>"` line there is followed
# by its `integrity: "sha384-..."` line. Only lines that start with the field name count, so doc
# comments that quote these fields (`/// ...`) are skipped. Prints "<name> <integrity>" per font.
font_list() {
  awk '
    /^[[:space:]]*url: "fonts\// { match($0, /fonts\/[^"]+/); name = substr($0, RSTART + 6, RLENGTH - 6); next }
    /^[[:space:]]*integrity: "sha384-/ && name != "" { match($0, /sha384-[^"]+/); print name, substr($0, RSTART, RLENGTH); name = "" }
  ' src/web.rs
}

# Downloads a font into the cache unless the cached copy already has the expected hash. Returns 1
# when the download fails; a wrong hash always stops the build, since then the code is wrong.
# It runs as an `if` condition, where `set -e` does not apply, so each step checks for itself.
fetch_font() {
  name=$1
  integrity=$2
  cached="$FONT_CACHE_DIR/$name"
  if [ -f "$cached" ] && [ "$(sri_sha384 "$cached")" = "$integrity" ]; then
    return 0
  fi
  if ! mkdir -p "$FONT_CACHE_DIR"; then
    echo "error: could not create $FONT_CACHE_DIR" >&2
    exit 1
  fi
  download="$cached.download"
  if ! curl --fail --silent --show-error --location --retry 3 --output "$download" "$FONT_BASE_URL/$name"; then
    rm -f "$download"
    return 1
  fi
  actual=$(sri_sha384 "$download")
  if [ "$actual" != "$integrity" ]; then
    rm -f "$download"
    echo "error: $FONT_BASE_URL/$name has $actual, but src/web.rs expects $integrity" >&2
    exit 1
  fi
  if ! mv "$download" "$cached"; then
    echo "error: could not move $download to $cached" >&2
    exit 1
  fi
}

fonts=$(font_list)
if [ -z "$fonts" ]; then
  echo "error: found no fonts in IBM_PLEX_SANS_JP in src/web.rs" >&2
  exit 1
fi

mkdir -p "$TRUNK_STAGING_DIR/fonts" "$TRUNK_STAGING_DIR/licenses"
while read -r name integrity; do
  if fetch_font "$name" "$integrity"; then
    cp "$FONT_CACHE_DIR/$name" "$TRUNK_STAGING_DIR/fonts/"
  elif [ "$is_release" = true ]; then
    echo "error: could not download $FONT_BASE_URL/$name" >&2
    exit 1
  else
    echo "warning: could not download $name; characters the bundled font lacks stay tofu" >&2
  fi
done <<EOF
$fonts
EOF

# Two of the sources are named LICENSE, so each gets its own name.
cp LICENSE "$TRUNK_STAGING_DIR/licenses/csvr-LICENSE.txt"
cp THIRD_PARTY_LICENSES.html "$TRUNK_STAGING_DIR/licenses/THIRD_PARTY_LICENSES.html"
# The OFL covers IBM Plex Sans (bundled in the wasm) and IBM Plex Sans JP (in fonts/).
cp assets/fonts/ibm-plex-sans/license.txt "$TRUNK_STAGING_DIR/licenses/IBM-Plex-OFL.txt"
# The favicon is the app icon, based on a Lucide icon.
cp assets/LICENSE "$TRUNK_STAGING_DIR/licenses/icon-LICENSE.txt"
# The welcome screen's link opens /licenses/. No inline script or style, so the CSP stays as it is;
# the color-scheme meta lets the browser follow a dark theme without one.
cat > "$TRUNK_STAGING_DIR/licenses/index.html" <<'HTML'
<!doctype html>
<html lang="ja">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <meta name="color-scheme" content="light dark" />
    <title>ライセンス - csvr</title>
  </head>
  <body>
    <h1>ライセンス</h1>
    <ul>
      <li><a href="csvr-LICENSE.txt">csvr</a>（MIT）</li>
      <li><a href="THIRD_PARTY_LICENSES.html">依存クレート</a></li>
      <li><a href="IBM-Plex-OFL.txt">IBM Plex Sans / IBM Plex Sans JP</a>（OFL）</li>
      <li><a href="icon-LICENSE.txt">アイコン</a>（Lucide、ISC）</li>
    </ul>
    <p><a href="../">csvr を開く</a></p>
  </body>
</html>
HTML

# Trunk keeps its lock file in the output directory; wrangler would otherwise publish it.
printf '.trunk-lock\n' > "$TRUNK_STAGING_DIR/.assetsignore"

# Only Cloudflare reads `_headers`, and only release builds are deployed, so `trunk serve` does not
# need Node. Last, because it hashes the inline code of every HTML file, the licenses page included.
if [ "$is_release" = true ]; then
  node scripts/web-headers.mjs "$TRUNK_STAGING_DIR"
fi
