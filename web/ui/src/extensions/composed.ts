/*
 * The composed extension entry (S-642, FR-UI-51) — what `virtual:logos-extensions`
 * resolves to in this build: the code of every extension the `logos` binary built from
 * this repository can compose. That binary composes no extension (FR-DS-05), so this
 * entry registers none, and the shell is exactly the built-in one.
 *
 * A build that composes extensions points the alias (`vite.config.ts`,
 * `vitest.config.ts`) at an entry of its own with the same shape: a default export of
 * the `ExtensionUi` registrations it contributes.
 */

import type { ExtensionUi } from "./registry.ts";

const registrations: readonly ExtensionUi[] = [];

export default registrations;
