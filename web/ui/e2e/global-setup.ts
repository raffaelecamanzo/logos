// Builds the widget harness page (e2e/harness/) before any spec runs (S-611).
// No view imports the widget frame yet, so the served SPA cannot render one; the
// harness is a separate Vite build of the SAME components and stylesheets, which
// widget-frame.project.spec.ts serves at the logos origin under the self-only CSP.
import { fileURLToPath } from "node:url";

import { build } from "vite";

export default async function globalSetup(): Promise<void> {
  await build({
    configFile: fileURLToPath(new URL("./harness/vite.config.ts", import.meta.url)),
    logLevel: "warn",
  });
}
