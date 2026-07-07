import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Vitest config for the SPA component/unit tests (S-186). Build-time-only, like
// the rest of the toolchain — it never ships in the embedded bundle. jsdom gives
// the React Testing Library a DOM; ECharts (which needs a real canvas) is mocked
// per-test via the `src/views/graph/echarts.ts` seam, so the canvas component is
// exercised without a headless-canvas dependency.
export default defineConfig({
  plugins: [react()],
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
