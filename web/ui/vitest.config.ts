import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

import { EXTENSIONS_ENTRY } from "./vite.config.ts";

// Vitest config for the SPA component/unit tests (S-186). Build-time-only, like
// the rest of the toolchain — it never ships in the embedded bundle. jsdom gives
// the React Testing Library a DOM; ECharts (which needs a real canvas) is mocked
// per-test via the `src/views/graph/echarts.ts` seam, so the canvas component is
// exercised without a headless-canvas dependency.
export default defineConfig({
  plugins: [react()],
  // The same alias as `vite.config.ts` (S-642), from the same constant:
  // `virtual:logos-extensions` is not a file, so vitest must resolve it too — to the
  // composed entry (the `chat` extension's registration, S-643); a test that needs a
  // different registration `vi.mock`s it.
  resolve: { alias: { "virtual:logos-extensions": EXTENSIONS_ENTRY } },
  test: {
    environment: "jsdom",
    globals: false,
    setupFiles: ["./src/test/setup.ts"],
    // CSS Modules are replaced with empty modules — tests assert on role/text/ARIA,
    // not class names, so no CSS processing is needed.
    css: false,
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
