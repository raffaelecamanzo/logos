/*
 * The extension manifest's data-access layer (S-642, FR-UI-51, CR-213) — a thin,
 * typed GET over the same-origin `/api/v1/extensions` endpoint (S-641).
 *
 * Like every read surface it goes through the shared `apiFetch` seam: same-origin,
 * GET-only, no intent token. The endpoint reads the extension host and nothing
 * else (no engine, no `?repo=` scope), and the SPA reads it ONCE at boot
 * (`extensions/boot.ts`): what the server reports active is what the shell mounts.
 *
 * The wire is snake case, like the Rust manifest it serialises (`logos-ext`'s
 * `UiManifest`); `replaced_in_workspace_by` and `dashboard_cards` are omitted when
 * absent and `ui` is `null` for an active extension that contributes no UI.
 */

import { apiFetch } from "./client.ts";

/** One sidebar entry — the SPA's `NavItem`, on the wire. */
export interface ExtensionNavEntry {
  id: string;
  label: string;
  path: string;
  group: string;
  scope: string;
  replaced_in_workspace_by?: string;
}

/** One config panel, editing the config table its extension owns. */
export interface ExtensionConfigPanel {
  id: string;
  label: string;
  table: string;
}

/** One Dashboard card, rendering a field its extension adds to the overview
 *  read-model (`GET /api/v1/overview`, S-644). */
export interface ExtensionDashboardCard {
  id: string;
  label: string;
}

/** Everything an active extension adds to the SPA. */
export interface ExtensionUiManifest {
  nav: ExtensionNavEntry[];
  /** The client routes the extension's views mount at (a nav entry's `path`, and
   *  any route a view owns without a sidebar entry of its own). */
  views: string[];
  config_panels: ExtensionConfigPanel[];
  /** Omitted on the wire when the extension adds none. */
  dashboard_cards?: ExtensionDashboardCard[];
}

/** One ACTIVE extension; `ui` is `null` when it contributes no manifest. */
export interface ExtensionInfo {
  id: string;
  ui: ExtensionUiManifest | null;
}

/** The `GET /api/v1/extensions` body. */
export interface ExtensionsInfo {
  extensions: ExtensionInfo[];
}

/**
 * `GET /api/v1/extensions` — the active extensions, each with its UI manifest. A
 * host with no extension answers `{"extensions":[]}`.
 */
export function fetchExtensions(): Promise<ExtensionsInfo> {
  return apiFetch<ExtensionsInfo>("extensions");
}
