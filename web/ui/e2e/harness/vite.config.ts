import { fileURLToPath } from "node:url";

import { mergeConfig } from "vite";

import appConfig from "../../vite.config.ts";

// The widget harness page (S-611): a Vite build of the real `Widget`,
// `WidgetStack` and `Term` with the real tokens and base stylesheet, for the
// Playwright layout spec. It INHERITS the app's config (plugins, the CSP-relevant
// build options) and overrides only where the page lives and where it is written,
// so the harness cannot drift from how the shipped SPA is built.
export default mergeConfig(appConfig, {
  root: fileURLToPath(new URL(".", import.meta.url)),
  base: "/__e2e/harness/",
  build: {
    outDir: fileURLToPath(new URL("../.harness-dist", import.meta.url)),
    emptyOutDir: true,
  },
});
