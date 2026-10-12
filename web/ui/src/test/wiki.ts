/*
 * Test seam for the `wiki` extension (S-644) — chat's twin (`test/chat.ts`): mount it
 * the way the SPA does in a build with the `wiki` feature — its bundle registration
 * (`views/wiki/extension.tsx`) and the manifest the server actually serves for it
 * (`extensions/fixtures/wiki.real.json`, generated and guarded by
 * `logos-wiki/tests/extension_surface.rs`). The suites whose subject is a view, card
 * or config field the wiki contributes call it before rendering; the registry's
 * `clearExtensions` puts the shell back to a build without the wiki.
 *
 * The shipped builds compose more than one extension, so their seams are here too:
 * the default build composes the wiki alone, the `agents` build chat and the wiki.
 */

import type { ExtensionsInfo } from "../api/extensionsClient.ts";
import served from "../extensions/fixtures/wiki.real.json";
import { registerExtensionUi, setActiveExtensions } from "../extensions/registry.ts";
import { CHAT_EXTENSION_UI } from "../views/chat/extension.tsx";
import { WIKI_EXTENSION_UI } from "../views/wiki/extension.tsx";
import { CHAT_ACTIVE } from "./chat.ts";

/** The `GET /api/v1/extensions` body a `logos` binary composing the wiki serves. */
export const WIKI_ACTIVE: ExtensionsInfo = served as ExtensionsInfo;

/** Register the wiki extension's code and report it active, as the boot read would —
 *  the default build's shell. */
export function mountWiki(): void {
  registerExtensionUi(WIKI_EXTENSION_UI);
  setActiveExtensions(WIKI_ACTIVE);
}

/** The `agents` build's shell: chat and the wiki registered, in the order the
 *  composed entry registers them (`extensions/composed.ts`), and both reported active
 *  in the order the binary composes them (chat, then the wiki —
 *  `cli/src/composition.rs`), which is the order their sidebar entries take after
 *  Graph. */
export function mountAgentsBuild(): void {
  registerExtensionUi(CHAT_EXTENSION_UI);
  registerExtensionUi(WIKI_EXTENSION_UI);
  setActiveExtensions({
    extensions: [...CHAT_ACTIVE.extensions, ...WIKI_ACTIVE.extensions],
  });
}
