import { fileURLToPath } from "node:url";

import { defineConfig, devices } from "@playwright/test";

// Browser tests for the web UI (S-611, CR-203 §11, FR-UI-40). Build-time-only,
// like the rest of the toolchain: nothing here ships in the embedded bundle.
//
// Each project drives a TREE-BUILT `logos serve --ui` over a checked-in fixture
// (e2e/fixtures/), so a spec reads computed style in a real browser — the one
// thing vitest cannot do, because its CSS modules are a proxy under `css: false`.
// The shell specs load the bundle the server serves. No view renders a `Widget`
// yet, so the frame spec loads a separate Vite build of the same components and
// stylesheets (e2e/harness/) at the server's origin, under the CSP header the
// server sends; the layout specs of the stories that convert views load the
// real views.
//
//   project   — a single repository: the project views.
//   workspace — a parent of two member repositories: the workspace views.
//
// A spec runs in the project its file name names (`*.project.spec.ts`,
// `*.workspace.spec.ts`). Chromium only. Run through `npm run test:e2e`, which
// scripts/gate.sh's `ui-e2e` leg calls in the FULL tier only; the binary must
// embed a real SPA build (`npm run build`, then `cargo build -p logos`).
const PROJECT_PORT = Number(process.env.LOGOS_E2E_PROJECT_PORT ?? 4991);
const WORKSPACE_PORT = Number(process.env.LOGOS_E2E_WORKSPACE_PORT ?? 4992);

// Absolute, so scripts/gate.sh can stop a stray fixture script of THIS tree by name.
const SERVE_FIXTURE = fileURLToPath(new URL("./e2e/serve-fixture.sh", import.meta.url));

const server = (kind: "single" | "workspace", port: number) => ({
  command: `bash "${SERVE_FIXTURE}" ${kind} ${port}`,
  url: `http://127.0.0.1:${port}/`,
  // Never adopt a server already on the port: it may be another tree's binary.
  reuseExistingServer: false,
  // Bounded: a server that never answers fails the run instead of hanging it.
  timeout: 120_000,
  gracefulShutdown: { signal: "SIGTERM" as const, timeout: 5_000 },
  stdout: "ignore" as const,
  stderr: "pipe" as const,
});

export default defineConfig({
  testDir: "e2e",
  outputDir: "test-results",
  globalSetup: "./e2e/global-setup.ts",
  forbidOnly: true,
  // Every wait is bounded, so a stalled page or server fails the leg with a
  // stated reason rather than hanging the gate: per test, per assertion, per
  // navigation/action, and for the whole run.
  timeout: 30_000,
  expect: { timeout: 5_000 },
  globalTimeout: 300_000,
  retries: 0,
  workers: 2,
  reporter: [["list"], ["json", { outputFile: "e2e/.results/results.json" }]],
  use: {
    ...devices["Desktop Chrome"],
    viewport: { width: 1280, height: 900 },
    trace: "retain-on-failure",
    actionTimeout: 10_000,
    navigationTimeout: 15_000,
  },
  projects: [
    {
      name: "project",
      testMatch: "**/*.project.spec.ts",
      use: { baseURL: `http://127.0.0.1:${PROJECT_PORT}` },
    },
    {
      name: "workspace",
      testMatch: "**/*.workspace.spec.ts",
      use: { baseURL: `http://127.0.0.1:${WORKSPACE_PORT}` },
    },
  ],
  webServer: [server("single", PROJECT_PORT), server("workspace", WORKSPACE_PORT)],
});
