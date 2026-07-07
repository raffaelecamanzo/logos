#!/usr/bin/env bash
# Vendor the web UI's third-party assets into web/assets/ (CR-012, FR-UI-02,
# NFR-CR-01, ADR-27).
#
# This is a DEVELOPMENT-TIME step, not a build or runtime step. It downloads the
# exact pinned upstream files once so they can be committed into the repository
# and embedded into the `logos` binary via `include_bytes!` (web/src/assets.rs).
# The shipped artifact fetches NOTHING at build or run time — the offline
# carve-out (ADR-27, BR-33) is intact; this script merely makes the vendored
# set reproducible and auditable. Updating an asset is a deliberate, reviewed
# replacement: bump the version below, re-run, review the diff, update VENDOR.md.
#
# Usage:  bash scripts/vendor-web-assets.sh
# Requires: curl, sha256sum (or shasum).
set -euo pipefail

# ── Pinned versions (the single source of truth for an update) ────────────────
HTMX_VER="2.0.4"        # BSD 2-Clause
UPLOT_VER="1.6.31"      # MIT
ECHARTS_VER="5.6.0"     # Apache-2.0 (graph-only custom build, tree-shaken via esbuild)
MERMAID_VER="10.9.3"    # MIT (UMD single-bundle; wiki diagram renderer, FR-WK-15)
INTER_VER="5.1.0"       # OFL-1.1 (@fontsource/inter)
JBMONO_VER="5.1.0"      # OFL-1.1 (@fontsource/jetbrains-mono)

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ASSETS="$ROOT/web/assets"
VENDOR="$ASSETS/vendor"
FONTS="$ASSETS/fonts"
LIC="$VENDOR/licenses"
mkdir -p "$VENDOR" "$FONTS" "$LIC"

sha() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }

fetch() { # url dest
  echo "  → $2"
  curl -fsSL --proto '=https' --tlsv1.2 -o "$2" "$1" \
    || { echo "FAILED: $1" >&2; exit 1; }
  [ -s "$2" ] || { echo "EMPTY: $2" >&2; exit 1; }
}

echo "Vendoring web assets into $ASSETS"

# ── htmx (fragment swaps) ─ BSD 2-Clause ──────────────────────────────────────
fetch "https://unpkg.com/htmx.org@${HTMX_VER}/dist/htmx.min.js" "$VENDOR/htmx.min.js"
fetch "https://unpkg.com/htmx.org@${HTMX_VER}/LICENSE"          "$LIC/htmx-LICENSE.txt"

# ── uPlot (charts) ─ MIT ──────────────────────────────────────────────────────
fetch "https://unpkg.com/uplot@${UPLOT_VER}/dist/uPlot.iife.min.js" "$VENDOR/uplot.min.js"
fetch "https://unpkg.com/uplot@${UPLOT_VER}/dist/uPlot.min.css"     "$VENDOR/uplot.min.css"
fetch "https://unpkg.com/uplot@${UPLOT_VER}/LICENSE"               "$LIC/uplot-LICENSE.txt"

# ── ECharts (graph canvas) ─ Apache-2.0 ───────────────────────────────────────
# Upstream publishes no prebuilt graph-only bundle, so we tree-shake one at
# vendor time: install echarts + esbuild in a scratch dir and bundle just the
# graph/network series + canvas renderer + the few components the canvas uses
# into one minified IIFE that exposes `window.echarts`. This is a dev-time step
# (requires node + npm); the shipped artifact still fetches nothing — the output
# is committed and embedded. Re-run to update: bump ECHARTS_VER, review the diff.
build_echarts_graph() { # version dest
  local ver="$1" dest="$2" scratch
  scratch="$(mktemp -d)"
  ( cd "$scratch"
    npm init -y >/dev/null 2>&1
    npm install "echarts@${ver}" esbuild >/dev/null 2>&1
    cat > entry.js <<'JS'
import * as echarts from 'echarts/core';
import { GraphChart } from 'echarts/charts';
import { TooltipComponent, LegendComponent, TitleComponent, DataZoomComponent, ToolboxComponent } from 'echarts/components';
import { CanvasRenderer } from 'echarts/renderers';
echarts.use([GraphChart, TooltipComponent, LegendComponent, TitleComponent, DataZoomComponent, ToolboxComponent, CanvasRenderer]);
if (typeof window !== 'undefined') { window.echarts = echarts; }
JS
    npx esbuild entry.js --bundle --minify --format=iife --legal-comments=none --outfile=echarts-graph.min.js >/dev/null 2>&1
  )
  cp "$scratch/echarts-graph.min.js" "$dest"
  cp "$scratch/node_modules/echarts/LICENSE" "$LIC/echarts-LICENSE.txt"
  rm -rf "$scratch"
  [ -s "$dest" ] || { echo "FAILED: echarts graph-only build" >&2; exit 1; }
}
echo "  → $VENDOR/echarts-graph.min.js (graph-only build via esbuild)"
build_echarts_graph "$ECHARTS_VER" "$VENDOR/echarts-graph.min.js"

# ── Mermaid (wiki diagram renderer) ─ MIT ─────────────────────────────────────
# A UMD single-file build that exposes `window.mermaid`; the wiki's same-origin
# init script renders every `.mermaid` block from it (FR-WK-15, ADR-27). The
# bundle already includes its own deps (d3/dagre/…) — no esbuild step needed.
fetch "https://unpkg.com/mermaid@${MERMAID_VER}/dist/mermaid.min.js" "$VENDOR/mermaid.min.js"
fetch "https://unpkg.com/mermaid@${MERMAID_VER}/LICENSE"             "$LIC/mermaid-LICENSE.txt"

# ── Inter WOFF2 (sans face; brand fallback) ─ OFL-1.1 ─────────────────────────
for w in 400 600 700; do
  fetch "https://unpkg.com/@fontsource/inter@${INTER_VER}/files/inter-latin-${w}-normal.woff2" "$FONTS/inter-${w}.woff2"
done
fetch "https://unpkg.com/@fontsource/inter@${INTER_VER}/LICENSE" "$LIC/inter-OFL.txt"

# ── JetBrains Mono WOFF2 (code/identifier face) ─ OFL-1.1 ─────────────────────
for w in 400 600; do
  fetch "https://unpkg.com/@fontsource/jetbrains-mono@${JBMONO_VER}/files/jetbrains-mono-latin-${w}-normal.woff2" "$FONTS/jetbrains-mono-${w}.woff2"
done
fetch "https://unpkg.com/@fontsource/jetbrains-mono@${JBMONO_VER}/LICENSE" "$LIC/jetbrains-mono-OFL.txt"

echo
echo "Vendored file checksums (record in web/assets/VENDOR.md):"
printf '%-40s %-10s %s\n' "FILE" "BYTES" "SHA256"
while IFS= read -r f; do
  printf '%-40s %-10s %s\n' "${f#$ASSETS/}" "$(wc -c < "$f" | tr -d ' ')" "$(sha "$f")"
done < <(find "$VENDOR" "$FONTS" -type f ! -path '*/licenses/*' | sort)

echo
echo "Versions: htmx=$HTMX_VER uplot=$UPLOT_VER echarts=$ECHARTS_VER mermaid=$MERMAID_VER inter=$INTER_VER jetbrains-mono=$JBMONO_VER"
echo "Done."
