/*
 * The extension UI registry (S-642, FR-UI-51, CR-213, ADR-74) — the SPA's half of
 * the extension host.
 *
 * Two things meet here, and NEITHER alone mounts anything:
 *
 *   - the CODE an extension contributes — its icons, view components, config-panel
 *     components, config field groups and Dashboard cards — registered at build
 *     time through the
 *     `virtual:logos-extensions` module (`boot.ts`); and
 *   - the SERVER's statement of which extensions are ACTIVE and what each puts in the
 *     UI (`GET /api/v1/extensions`, read once at boot).
 *
 * An entry is mounted only when the server reports its extension active AND lists
 * the entry in that extension's manifest AND the bundle carries the code for it.
 * Code without a manifest line is inert (an extension compiled into the bundle but
 * inactive for this serve), and a manifest line without code is skipped rather than
 * rendered as a link to nothing. A server that reports no extension active leaves
 * every list below empty, and the shell is exactly the built-in one.
 *
 * `nav.ts`, `views/index.ts`, the sidebar's icon map, the two Config views and the
 * Dashboard consult this module at call time; none of them keeps a second list. This module
 * imports nothing from `nav.ts` or the views, so `nav.ts` — which the Playwright specs
 * load directly under node — pulls in this file, `react` and the `routeOwns` leaf,
 * never the view components or `virtual:logos-extensions`.
 */

import { useSyncExternalStore, type ComponentType, type SVGProps } from "react";

import type {
  ExtensionConfigPanel,
  ExtensionDashboardCard,
  ExtensionInfo,
  ExtensionNavEntry,
  ExtensionsInfo,
} from "../api/extensionsClient.ts";
import type { ConfigReadModel, OverviewModel } from "../api/types.ts";
import { routeOwns } from "../routeOwns.ts";

/** A view: a plain, prop-less component mounted in the shell content slot — the shape
 *  `views/index.ts` calls `ViewComponent`, spelled here so this module imports nothing
 *  from the views (`views/index.ts` imports this one, and a file-level cycle between
 *  them is exactly what the architecture gate's acyclicity measures). */
type ViewComponent = ComponentType;

export type IconComponent = ComponentType<SVGProps<SVGSVGElement>>;

/** Where a config panel is offered: a member's Config view, or the workspace's. */
export type ConfigPanelScope = "member" | "workspace";

/**
 * What a Config view hands the panels it mounts beside its own editors: the
 * read-model it loaded, and the latest re-read of it. The member Config view re-reads
 * after every save, because a save can change the resolution a panel states (a member
 * that declares a key of its own stops inheriting the workspace's); `fresh` is that
 * re-read, or `null` while the last one failed — a panel then drops claims about the
 * resolution rather than keep asserting one this page no longer knows to be current.
 * The workspace Config view hands none: its panels own their reads.
 */
export interface ConfigPanelHost {
  model: ConfigReadModel;
  fresh: ConfigReadModel | null;
  /** Re-read the read-model, after a panel's own save. */
  refresh: () => void;
}

/** The props a config-panel component receives: the manifest line that mounted it,
 *  and its Config view's host context where that view has one. */
export interface ConfigPanelProps {
  panel: ExtensionConfigPanel;
  host?: ConfigPanelHost;
}

/** The TOML shape a config field edits. A `choice` is a string drawn from its
 *  options; the rest are the shapes the core's typed fields take. */
export type ConfigFieldKind = "int" | "float" | "bool" | "list" | "str" | "choice";

/** What every config field declares, whatever its kind. */
interface ConfigFieldBase {
  /** The `config.toml` table holding the key: the table of the config panel the
   *  field's group is keyed to, or a sub-table of it. */
  table: string;
  key: string;
  label: string;
  help: string;
  placeholder?: string;
  /** The default the server applies when the key is unset, as the field states it
   *  beside the value ("Default: …"). */
  defaultHint?: string;
}

/**
 * One typed field an extension contributes to a Config form (S-730, FR-UI-54). Its
 * value is read from, and patched into, the candidate document the form already
 * edits, so the field is never a second writer of the file.
 */
export type ConfigFieldDescriptor =
  | (ConfigFieldBase & { kind: Exclude<ConfigFieldKind, "choice"> })
  | (ConfigFieldBase & { kind: "choice"; options: readonly string[] });

/**
 * A group of config fields, rendered after the Config form's core groups. Its
 * `note` is read-only text above the fields: a fixed string, or one computed from
 * the view's host context ({@link ConfigPanelHost}; the member view hands one, the
 * workspace view none) — `null` renders no note.
 */
export interface ConfigFieldGroup {
  legend?: string;
  note?: string | ((host: ConfigPanelHost | undefined) => string | null);
  fields: readonly ConfigFieldDescriptor[];
}

/** The props a Dashboard card receives (S-644): the manifest line that mounted it,
 *  and the Dashboard's overview read-model — which carries the field the card's
 *  extension adds to it (`OverviewField` on the server), so the card reads from the
 *  Dashboard's one request rather than a route of its own. */
export interface DashboardCardProps {
  card: ExtensionDashboardCard;
  overview: OverviewModel;
}

/**
 * The code one extension contributes to the SPA. Everything is keyed by the id or
 * path its server-side manifest uses, which is the only join between the two halves.
 */
export interface ExtensionUi {
  /** The extension's id — the server's `Extension::id`. */
  id: string;
  /** Sidebar icon per navigation id. */
  icons?: Readonly<Record<string, IconComponent>>;
  /** View component per client route. */
  views?: Readonly<Record<string, ViewComponent>>;
  /** Config-panel component per panel id, per Config view it appears in. */
  configPanels?: Readonly<
    Record<string, Partial<Record<ConfigPanelScope, ComponentType<ConfigPanelProps>>>>
  >;
  /** Config field groups per panel id, per Config view they appear in (S-730). A
   *  group is offered only when the manifest lists its panel, like the panel's
   *  component, and a field outside the panel's table is not offered at all: the
   *  manifest names the tables an extension edits. */
  configFields?: Readonly<
    Record<string, Partial<Record<ConfigPanelScope, readonly ConfigFieldGroup[]>>>
  >;
  /** Dashboard-card component per card id (S-644). Where the cards render is the
   *  Dashboard's: one slot, directly after its freshness callout. */
  dashboardCards?: Readonly<Record<string, ComponentType<DashboardCardProps>>>;
  /**
   * Where a navigation entry sits: the id of the entry it follows in its sidebar
   * section, per navigation id. Placement is the bundle's to declare, like a config
   * panel's scope — the manifest says WHAT is offered, the code where it renders —
   * so an extension that ships a view the built-ins surround (one anchored after
   * Graph, say) keeps its place. An entry with none, or whose anchor is not
   * offered, is appended (`nav.ts`).
   */
  navAfter?: Readonly<Record<string, string>>;
}

/** One config panel that is mounted, with the component that renders it. */
export interface MountedConfigPanel {
  scope: ConfigPanelScope;
  panel: ExtensionConfigPanel;
  Component: ComponentType<ConfigPanelProps>;
}

/** One config field group that is mounted, with the panel that keyed it. */
export interface MountedConfigFieldGroup {
  scope: ConfigPanelScope;
  panel: ExtensionConfigPanel;
  group: ConfigFieldGroup;
}

/** One Dashboard card that is mounted, with the component that renders it. */
export interface MountedDashboardCard {
  card: ExtensionDashboardCard;
  Component: ComponentType<DashboardCardProps>;
}

/** What one ACTIVE, registered extension contributes to this serve. */
export interface MountedExtension {
  id: string;
  nav: readonly ExtensionNavEntry[];
  /** The anchor each mounted navigation entry follows ({@link ExtensionUi.navAfter}). */
  navAfter: Readonly<Record<string, string>>;
  icons: Readonly<Record<string, IconComponent>>;
  views: Readonly<Record<string, ViewComponent>>;
  configPanels: readonly MountedConfigPanel[];
  configFields: readonly MountedConfigFieldGroup[];
  dashboardCards: readonly MountedDashboardCard[];
}

export interface ExtensionsSnapshot {
  /** `loading` until the one boot read settles; a failed read settles as `ready`
   *  with nothing active (an extension the server has not vouched for is not mounted). */
  status: "loading" | "ready";
  mounted: readonly MountedExtension[];
  /** Every route any REGISTERED extension owns, mounted or not. */
  registeredPaths: readonly string[];
}

const registrations = new Map<string, ExtensionUi>();
let active: readonly ExtensionInfo[] = [];
let status: ExtensionsSnapshot["status"] = "loading";
let snapshot: ExtensionsSnapshot = compose();
const listeners = new Set<() => void>();

function mount(ext: ExtensionInfo, reg: ExtensionUi): MountedExtension | null {
  const ui = ext.ui;
  if (ui === null) return null;
  // Navigation comes from the manifest, and only for an entry the bundle can render:
  // the route needs a registered view, or the link would lead nowhere.
  const nav = ui.nav.filter((entry) => reg.views?.[entry.path] !== undefined);
  const listed = new Set([...ui.views, ...nav.map((n) => n.path)]);
  const views = Object.fromEntries(
    Object.entries(reg.views ?? {}).filter(([path]) => listed.has(path)),
  );
  const icons = Object.fromEntries(
    nav.flatMap((n) => (reg.icons?.[n.id] !== undefined ? [[n.id, reg.icons[n.id]]] : [])),
  );
  const configPanels = ui.config_panels.flatMap((panel) =>
    (["member", "workspace"] as const).flatMap((scope) => {
      const Component = reg.configPanels?.[panel.id]?.[scope];
      return Component === undefined ? [] : [{ scope, panel, Component }];
    }),
  );
  // Field groups keyed to a panel the manifest lists, keeping only the fields in
  // that panel's table (or a sub-table of it); a group left with none is dropped.
  const configFields = ui.config_panels.flatMap((panel) =>
    (["member", "workspace"] as const).flatMap((scope) =>
      (reg.configFields?.[panel.id]?.[scope] ?? []).flatMap((group) => {
        const fields = group.fields.filter((f) => inTable(f.table, panel.table));
        return fields.length === 0 ? [] : [{ scope, panel, group: { ...group, fields } }];
      }),
    ),
  );
  // A card the manifest lists AND the bundle can render, like a config panel.
  const dashboardCards = (ui.dashboard_cards ?? []).flatMap((card) => {
    const Component = reg.dashboardCards?.[card.id];
    return Component === undefined ? [] : [{ card, Component }];
  });
  const navAfter = Object.fromEntries(
    nav.flatMap((n) => (reg.navAfter?.[n.id] !== undefined ? [[n.id, reg.navAfter[n.id]]] : [])),
  );
  return { id: ext.id, nav, navAfter, icons, views, configPanels, configFields, dashboardCards };
}

/** Is `table` the panel's table `owner`, or a sub-table of it? */
function inTable(table: string, owner: string): boolean {
  return owner !== "" && (table === owner || table.startsWith(`${owner}.`));
}

function compose(): ExtensionsSnapshot {
  const seen = new Set<string>();
  const mounted: MountedExtension[] = [];
  for (const ext of active) {
    const reg = registrations.get(ext.id);
    if (reg === undefined || seen.has(ext.id)) continue;
    seen.add(ext.id);
    const m = mount(ext, reg);
    if (m !== null) mounted.push(m);
  }
  const registeredPaths = [...registrations.values()].flatMap((r) => Object.keys(r.views ?? {}));
  return { status, mounted, registeredPaths };
}

function publish(): void {
  snapshot = compose();
  listeners.forEach((l) => l());
}

/** Register the code one extension contributes. A second registration under one id
 *  is a build defect, so it throws instead of silently replacing the first. */
export function registerExtensionUi(ui: ExtensionUi): void {
  if (registrations.has(ui.id)) {
    throw new Error(`extensions: "${ui.id}" is registered twice`);
  }
  registrations.set(ui.id, ui);
  publish();
}

/** Record the boot read. `null` is a failed read: nothing is active. */
export function setActiveExtensions(info: ExtensionsInfo | null): void {
  active = info?.extensions ?? [];
  status = "ready";
  publish();
}

/** Forget every registration and the boot read — the tests' way back to the
 *  just-loaded state. Never called by the shipped bundle. */
export function clearExtensions(): void {
  registrations.clear();
  active = [];
  status = "loading";
  publish();
}

export function getExtensions(): ExtensionsSnapshot {
  return snapshot;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => void listeners.delete(listener);
}

/** The current snapshot, re-rendering the caller when the boot read settles. */
export function useExtensions(): ExtensionsSnapshot {
  return useSyncExternalStore(subscribe, getExtensions);
}

/** Every mounted extension navigation entry as the server reports it, in
 *  registration-then-manifest order. `nav.ts` turns them into `NavItem`s: it owns the
 *  vocabulary (groups, scopes) an entry must speak, so this module keeps no copy. */
export function mountedNavEntries(): readonly ExtensionNavEntry[] {
  return snapshot.mounted.flatMap((m) => m.nav);
}

/** The navigation id the mounted entry `id` follows, if its extension placed it. */
export function mountedNavAnchor(id: string): string | undefined {
  for (const m of snapshot.mounted) {
    if (m.navAfter[id] !== undefined) return m.navAfter[id];
  }
  return undefined;
}

/** Every mounted view, as `[route, component]`. */
export function mountedViews(): readonly (readonly [string, ViewComponent])[] {
  return snapshot.mounted.flatMap((m) => Object.entries(m.views));
}

/** The icon a mounted extension gives navigation id `id`, if any. */
export function mountedIcon(id: string): IconComponent | undefined {
  for (const m of snapshot.mounted) {
    if (m.icons[id] !== undefined) return m.icons[id];
  }
  return undefined;
}

/** The mounted config panels offered in the Config view of `scope`. */
export function mountedConfigPanels(scope: ConfigPanelScope): readonly MountedConfigPanel[] {
  return snapshot.mounted.flatMap((m) => m.configPanels.filter((p) => p.scope === scope));
}

/** The mounted config field groups offered in the Config form of `scope` (S-730),
 *  in registration-then-manifest order. */
export function mountedConfigFields(scope: ConfigPanelScope): readonly MountedConfigFieldGroup[] {
  return snapshot.mounted.flatMap((m) => m.configFields.filter((g) => g.scope === scope));
}

/** The mounted Dashboard cards, in registration-then-manifest order (S-644). */
export function mountedDashboardCards(): readonly MountedDashboardCard[] {
  return snapshot.mounted.flatMap((m) => m.dashboardCards);
}

/** Does `pathname` fall under a route a REGISTERED extension owns — exactly, or as a
 *  sub-route (`routeOwns`)? Active or not. */
export function ownsRegisteredPath(pathname: string): boolean {
  return snapshot.registeredPaths.some((route) => routeOwns(route, pathname));
}
