import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Logos SPA build config — CSP-clean by construction (CR-049, FR-UI-22, ADR-43,
// ADR-44).
//
// The shipped bundle is embedded into the `logos` binary (web/ui/dist/ via
// rust-embed) and served same-origin under the *unchanged* self-only CSP
// (`default-src 'self'`, no `unsafe-inline`/`unsafe-eval`). Every option below
// exists to keep that CSP byte-identical:
//
//   - assetsInlineLimit: 0  — never inline an asset as a base64 `data:` URI, so
//     the bundle names only same-origin `/assets/*` URLs (no `data:` source the
//     CSP would have to permit).
//   - modulePreload.polyfill: false — Vite's modulepreload polyfill is the one
//     INLINE <script> it would otherwise inject into index.html; disabling it
//     keeps index.html free of any inline script (the self-only CSP forbids it).
//   - cssCodeSplit: true (default) — CSS is extracted to external `<link>`-ed
//     files, never injected as an inline <style> (no CSS-in-JS that emits a
//     <style> tag — ADR-44 bans runtime CSS-in-JS).
//   - base: "/" — assets are referenced as absolute same-origin `/assets/*`
//     paths, matching the loopback origin the shell is served from; the embed
//     layer maps `/assets/<f>` → web/ui/dist/assets/<f>.
//   - target: a modern baseline so the output uses native ES modules with no
//     SystemJS/eval shim (no `unsafe-eval`).
//
// The dev server (`npm run dev`) proxies /api and /chat to a running
// `logos serve --ui`; HMR/inline only ever exist in dev and never ship.
// /assets is NOT proxied: the vendored Mermaid bundle lives in public/assets/vendor/
// (S-196) and is served directly by Vite in dev mode.
export default defineConfig({
  plugins: [react()],
  base: "/",
  // Drop bundled dependencies' inline legal/license banner comments (e.g. ECharts/
  // zrender, which embed a `github.com` license URL). The bundle must name NO
  // external origin — the no-egress fitness test (web/tests/spa_bundle.rs) forbids
  // even a doc/license URL string (FR-UI-22). License attribution for the bundled
  // tree lives in THIRD-PARTY-NOTICES.md (NFR-CR-01), not in inline banners, so
  // stripping them keeps the bundle self-contained without losing compliance.
  esbuild: { legalComments: "none" },
  build: {
    outDir: "dist",
    assetsDir: "assets",
    // No base64-inlined assets: the bundle names only same-origin /assets/* URLs.
    assetsInlineLimit: 0,
    // External <link> stylesheets only — never an injected inline <style>.
    cssCodeSplit: true,
    // Modern browsers only (the audience is a developer on `logos serve --ui`):
    // native ESM, no SystemJS/eval transform that would require unsafe-eval.
    target: "es2022",
    modulePreload: {
      // The modulepreload polyfill is injected as an inline <script>; the
      // self-only CSP forbids inline scripts, so disable it. Browsers in our
      // modern target support <link rel="modulepreload"> natively.
      polyfill: false,
    },
    // Deterministic, reasonable chunking; a manifest aids the size budget check.
    manifest: true,
    sourcemap: false,
  },
  server: {
    // Frontend dev loop: proxy the same-origin API/chat/asset seams to a running
    // `logos serve --ui` so the SPA talks to a real engine during development.
    proxy: {
      "/api": "http://127.0.0.1:4983",
      "/chat": "http://127.0.0.1:4983",
      // /assets is NOT proxied: the vendored Mermaid bundle is now in
      // public/assets/vendor/ (S-196, FR-WK-15) — Vite serves it directly in dev
      // mode and Vite's build copies it verbatim to dist/assets/vendor/ for
      // production (served by the binary's spa_fallback → spa::asset).
    },
  },
});
