/*
 * Test seam for the `chat` extension (S-643): mount it the way the SPA does in the
 * `agents` build — its bundle registration (`views/chat/extension.tsx`) and the
 * manifest the server actually serves for it (`extensions/fixtures/chat.real.json`,
 * generated and guarded by `logos-chat/tests/extension_surface.rs`). The suites
 * whose subject is a view or panel chat contributes call it before rendering; the
 * registry's `clearExtensions` puts the shell back to a build without chat.
 */

import type { ExtensionsInfo } from "../api/extensionsClient.ts";
import served from "../extensions/fixtures/chat.real.json";
import { registerExtensionUi, setActiveExtensions } from "../extensions/registry.ts";
import { CHAT_EXTENSION_UI } from "../views/chat/extension.tsx";

/** The `GET /api/v1/extensions` body a `logos` binary built with `agents` serves. */
export const CHAT_ACTIVE: ExtensionsInfo = served as ExtensionsInfo;

/** Register the chat extension's code and report it active, as the boot read would. */
export function mountChat(): void {
  registerExtensionUi(CHAT_EXTENSION_UI);
  setActiveExtensions(CHAT_ACTIVE);
}
