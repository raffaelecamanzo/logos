/*
 * The composed extension entry (S-642, FR-UI-51) — what `virtual:logos-extensions`
 * resolves to in this build: the code of every extension the public `logos` binary
 * can compose — the `chat` extension's (S-643) and the `wiki` extension's (S-644).
 *
 * The bundle is one artifact embedded in every build, so it carries that code whether
 * or not the serving binary registers the extension. Registered code mounts nothing on
 * its own: an entry is mounted only when the server reports its extension active
 * (`registry.ts`), so a build without `agents` — no `chat` extension on the server —
 * shows no chat, and one without the `wiki` feature no wiki: the built-in shell.
 *
 * A build that composes other extensions points the alias (`vite.config.ts`,
 * `vitest.config.ts`) at an entry of its own with the same shape: a default export of
 * the `ExtensionUi` registrations it contributes.
 */

import { CHAT_EXTENSION_UI } from "../views/chat/extension.tsx";
import { WIKI_EXTENSION_UI } from "../views/wiki/extension.tsx";
import type { ExtensionUi } from "./registry.ts";

const registrations: readonly ExtensionUi[] = [CHAT_EXTENSION_UI, WIKI_EXTENSION_UI];

export default registrations;
