import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

import { EXTENSIONS_ENTRY } from "./vite.config.ts";

// The catalogue-command checks (S-617) read the command reference
// `docs/howto/commands.md` as `?raw`, from outside this project root. Vitest 4 loads
// modules through Vite's server, whose `server.fs` sandbox refuses such a file
// ("Denied ID"), so that one directory is allowed. Setting `allow` replaces Vite's
// default, so the root is listed too. Decoded as in `vite.config.ts`. The legacy
// Config editor script `web/assets/config-editor.js` is read the same way, by the
// `escapeRe` test in `src/views/config/toml.test.ts`.
const UI_ROOT = decodeURIComponent(new URL("./", import.meta.url).pathname);
const DOCS_HOWTO = decodeURIComponent(new URL("../../docs/howto/", import.meta.url).pathname);
const WEB_ASSETS = decodeURIComponent(new URL("../assets/", import.meta.url).pathname);

// Vitest config for the SPA component/unit tests (S-186). Build-time-only, like
// the rest of the toolchain — it never ships in the embedded bundle. jsdom gives
// the React Testing Library a DOM; ECharts (which needs a real canvas) is mocked
// per-test via the `src/views/graph/echarts.ts` seam, so the canvas component is
// exercised without a headless-canvas dependency. The `*.rendered.test.tsx` files
// run the real ECharts against a recording 2D context instead
// (`src/test/renderedChart.ts`).
export default defineConfig({
  plugins: [react()],
  // The same alias as `vite.config.ts` (S-642), from the same constant:
  // `virtual:logos-extensions` is not a file, so vitest must resolve it too — to the
  // composed entry, which registers no extension; a test that needs a registration
  // registers a stub (`src/test/extensions.tsx`) or `vi.mock`s it.
  resolve: { alias: { "virtual:logos-extensions": EXTENSIONS_ENTRY } },
  server: { fs: { allow: [UI_ROOT, DOCS_HOWTO, WEB_ASSETS] } },
  test: {
    environment: "jsdom",
    globals: false,
    setupFiles: ["./src/test/setup.ts"],
    // No CSS is processed: a CSS Module import is a proxy that returns a hashed name
    // for ANY key (`styles.widget` → `_widget_<hash>`), so a test can assert which
    // class an element carries but not that the stylesheet defines it.
    css: false,
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
