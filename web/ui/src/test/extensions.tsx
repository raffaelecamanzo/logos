/*
 * Test seam for the extension registry (S-642, FR-UI-51): a stub extension — the code
 * it would register through `virtual:logos-extensions` and the manifest the server
 * would report for it — plus the wire shapes the registry reads.
 *
 * The stub uses its own ids and routes, so nothing here can be mistaken for a built-in
 * entry; the collision tests build their own entries on purpose.
 */

import type { ExtensionInfo, ExtensionNavEntry, ExtensionsInfo } from "../api/extensionsClient.ts";
import type { ExtensionUi } from "../extensions/registry.ts";

export const STUB_ID = "stub";

export function StubIcon() {
  return <svg data-testid="stub-icon" />;
}
export function StubView() {
  return <div data-testid="stub-view">stub view</div>;
}
export function StubAppView() {
  return <div data-testid="stub-app-view">stub app view</div>;
}
export function StubMemberPanel({ panel }: { panel: { id: string; label: string; table: string } }) {
  return <section data-testid="stub-member-panel">{panel.label} [{panel.table}] (member)</section>;
}
export function StubWorkspacePanel({ panel }: { panel: { id: string; label: string; table: string } }) {
  return <section data-testid="stub-workspace-panel">{panel.label} [{panel.table}] (workspace)</section>;
}

/** The sidebar entry the stub's manifest reports. */
export const STUB_NAV: ExtensionNavEntry = {
  id: "stub",
  label: "Stub",
  path: "/stub",
  group: "A",
  scope: "member",
};

/** The code the stub registers. */
export const STUB_UI: ExtensionUi = {
  id: STUB_ID,
  icons: { stub: StubIcon },
  views: { "/stub": StubView, "/stub-app": StubAppView },
  configPanels: { "stub-config": { member: StubMemberPanel, workspace: StubWorkspacePanel } },
};

/** The stub as the server reports it active: one nav entry, its view, one config panel. */
export function stubInfo(over: Partial<ExtensionInfo> = {}): ExtensionInfo {
  return {
    id: STUB_ID,
    ui: {
      nav: [STUB_NAV],
      views: ["/stub"],
      config_panels: [{ id: "stub-config", label: "Stub config", table: "stub" }],
    },
    ...over,
  };
}

/** A `GET /api/v1/extensions` body listing `extensions`. */
export function extensionsInfo(...extensions: ExtensionInfo[]): ExtensionsInfo {
  return { extensions };
}
