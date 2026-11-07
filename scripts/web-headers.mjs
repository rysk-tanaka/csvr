// Writes `_headers` for Cloudflare Workers Static Assets into the browser build's output.
//
//   node scripts/web-headers.mjs web/dist
//
// Run it after `trunk build`. Trunk inlines the script that starts the app, and that script names
// the hashed wasm and JS files, so its CSP hash changes with every build.

import { createHash } from "node:crypto";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, relative } from "node:path";

const dist = process.argv[2];
if (!dist) {
  console.error("usage: node scripts/web-headers.mjs <trunk output directory>");
  process.exit(2);
}

/** All HTML files under `dir`, e.g. index.html and licenses/THIRD_PARTY_LICENSES.html. */
function htmlFiles(dir) {
  return readdirSync(dir, { withFileTypes: true, recursive: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith(".html"))
    .map((entry) => join(entry.parentPath, entry.name));
}

function cspHash(text) {
  return `'sha256-${createHash("sha256").update(text, "utf8").digest("base64")}'`;
}

/** CSP hashes of the inline scripts (those without `src`) and inline styles in `html`. */
function inlineHashes(html) {
  const scripts = [...html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/gi)]
    .filter(([, attributes]) => !/\bsrc\s*=/i.test(attributes))
    .map(([, , body]) => cspHash(body));
  const styles = [...html.matchAll(/<style\b[^>]*>([\s\S]*?)<\/style>/gi)].map(([, body]) =>
    cspHash(body),
  );
  return { scripts, styles };
}

const scriptHashes = new Set();
const styleHashes = new Set();
for (const file of htmlFiles(dist)) {
  const { scripts, styles } = inlineHashes(readFileSync(file, "utf8"));
  // Without these the app would not start under the CSP; their absence means Trunk's output
  // changed shape and this script needs updating.
  const isIndex = relative(dist, file) === "index.html";
  if (isIndex && (scripts.length === 0 || styles.length === 0)) {
    console.error(`${file}: expected an inline script and an inline style from Trunk`);
    process.exit(1);
  }
  for (const hash of scripts) scriptHashes.add(hash);
  for (const hash of styles) styleHashes.add(hash);
}

// `_headers` merges the headers of every rule that matches, so one CSP has to cover every page.
const csp = [
  "default-src 'none'",
  // Instantiating the wasm needs 'wasm-unsafe-eval'.
  `script-src 'self' 'wasm-unsafe-eval' ${[...scriptHashes].join(" ")}`,
  `style-src 'self' ${[...styleHashes].join(" ")}`,
  // gpui_web styles its safe-area probes through the `style` attribute (`safe_area_probe` in
  // gpui_web/src/viewport.rs). Per-value hashes would stop matching silently when a GPUI update
  // changes the values. The page never inserts data as HTML, and attributes cannot run scripts.
  "style-src-attr 'unsafe-inline'",
  // The Japanese font is fetched from this site (`fonts/`).
  "connect-src 'self'",
  "img-src 'self'",
  "base-uri 'none'",
  "form-action 'none'",
  "frame-ancestors 'none'",
].join("; ");

// Files Trunk names by their content can be kept for good.
const immutable = "public, max-age=31536000, immutable";
const headers = `/*
  Content-Security-Policy: ${csp}
  X-Content-Type-Options: nosniff
  Referrer-Policy: no-referrer

/csvr-*
  Cache-Control: ${immutable}

/icon-*
  Cache-Control: ${immutable}

/fonts/*
  Cache-Control: public, max-age=604800
`;

writeFileSync(join(dist, "_headers"), headers);
console.log(
  `wrote ${join(dist, "_headers")} (${scriptHashes.size} script and ${styleHashes.size} style hashes)`,
);
