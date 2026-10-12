/*
 * The extension boot (S-642, FR-UI-51, CR-213): register the code the build composed
 * in through `virtual:logos-extensions`, then read `GET /api/v1/extensions` ONCE and
 * hand the answer to the registry. Called from `main.tsx` before the first render and
 * from nowhere else — the shell never re-reads it, so an extension that goes inactive
 * mid-session is noticed at the next load, like every other boot-time fact.
 *
 * A failed read is not an error the shell can act on: it settles the registry as
 * "nothing active", so no extension entry is mounted that the server did not vouch
 * for, and the built-in navigation is untouched. A refused registration settles it
 * the same way, then rethrows.
 */

import registrations from "virtual:logos-extensions";

import { fetchExtensions } from "../api/extensionsClient.ts";
import { registerExtensionUi, setActiveExtensions } from "./registry.ts";

export async function bootExtensions(): Promise<void> {
  try {
    registrations.forEach(registerExtensionUi);
  } catch (defect) {
    // A registration is refused only for a build defect (one id twice). Say so loudly,
    // but settle first: an unsettled read would hold every registered extension route
    // on "Starting…" for good, and nothing is mounted without the server's answer.
    setActiveExtensions(null);
    throw defect;
  }
  try {
    setActiveExtensions(await fetchExtensions());
  } catch {
    setActiveExtensions(null);
  }
}
