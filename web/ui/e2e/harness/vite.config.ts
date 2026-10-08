import { fileURLToPath } from "node:url";

import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// The widget harness page (S-611): a Vite build of the real `Widget`,
// `WidgetStack` and `Term` with the real tokens and base stylesheet, for the
// Playwright layout spec. The CSP-relevant options mirror web/ui/vite.config.ts
// (no inlined assets, no modulepreload polyfill), so the page runs under the
// same self-only policy the served SPA does.
export default defineConfig({
  root: fileURLToPath(new URL(".", import.meta.url)),
  base: "/__e2e/harness/",
  plugins: [react()],
  esbuild: { legalComments: "none" },
  build: {
    outDir: fileURLToPath(new URL("../.harness-dist", import.meta.url)),
    emptyOutDir: true,
    assetsInlineLimit: 0,
    cssCodeSplit: true,
    target: "es2022",
    modulePreload: { polyfill: false },
    sourcemap: false,
  },
});
