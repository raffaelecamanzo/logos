/*
 * The config panels the mounted extensions add to a Config view (S-642, FR-UI-51).
 *
 * `ConfigView` (a member's Config) and `WorkspaceConfigView` (the workspace's) each
 * end their widget stack with one of these, instead of each keeping a list of
 * extension panels of its own: the registry decides which panels are mounted
 * (`extensions/registry.ts`), this only renders them. A panel is a component handed
 * the manifest line that mounted it (and its Config view's host context, where the
 * view has one) and renders its own `Widget`, like every panel beside it. Renders
 * nothing when no extension is mounted.
 */

import {
  mountedConfigPanels,
  useExtensions,
  type ConfigPanelHost,
  type ConfigPanelScope,
} from "./registry.ts";

export function ExtensionConfigPanels({ scope, host }: { scope: ConfigPanelScope; host?: ConfigPanelHost }) {
  // Subscribe, so the panels appear when the boot read of the active set settles.
  useExtensions();
  return (
    <>
      {mountedConfigPanels(scope).map(({ panel, Component }) => (
        <Component key={`${panel.table}/${panel.id}`} panel={panel} host={host} />
      ))}
    </>
  );
}

/** Does a mounted panel of `scope` edit the `config.toml` table `table`? A Config view
 *  asks before offering that table's typed fields in its document form, so a table no
 *  active extension edits gets none (S-643). Subscribes, like the panels. */
export function useMountedConfigTable(scope: ConfigPanelScope, table: string): boolean {
  useExtensions();
  return mountedConfigPanels(scope).some(({ panel }) => panel.table === table);
}
