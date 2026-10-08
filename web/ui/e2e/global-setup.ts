// Builds the widget harness page (e2e/harness/) before any spec runs (S-611).
// The harness is a fixed page of every frame state, a separate Vite build of the
// SAME components and stylesheets the served SPA uses, which
// widget-frame.project.spec.ts serves at the logos origin under the self-only CSP.
import { fileURLToPath } from "node:url";

import { build } from "vite";

export default async function globalSetup(): Promise<void> {
  await build({
    configFile: fileURLToPath(new URL("./harness/vite.config.ts", import.meta.url)),
    logLevel: "warn",
  });
}
