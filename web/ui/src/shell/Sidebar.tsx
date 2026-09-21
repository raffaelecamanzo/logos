/*
 * The shell sidebar (S-185, re-skinned onto the design system in S-193). Renders
 * the navigable views grouped into the three sidebar groups, each with an
 * idiomatic inline-SVG icon (frontend-design §3, CR-042). Every tab is a React
 * view: clicking pushes client-side history (router `navigate`) without a full
 * reload. The active item gets the rotated card-accent grammar (3px red left bar +
 * semibold) from Sidebar.module.css; the global :focus-visible ring marks keyboard
 * focus.
 *
 * S-425 (FR-UI-35, ADR-66) renders the scope each view declares. In WORKSPACE mode
 * the groups sit inside two labelled sections — Workspace (app-scoped) and Service
 * (member-scoped) — and the member selector sits in the Service section's header,
 * inside the boundary it governs and nowhere else. The CR-042 groups are untouched
 * by this: they keep their order INSIDE the Service section.
 *
 * In SINGLE-ROOT mode none of that is rendered: not a hidden section, not an empty
 * header — a plain repository has one scope, so a section label there would assert
 * an axis that does not exist (ADR-52, ADR-66 §5). That mode takes the early return
 * below, whose tree is the pre-S-425 one element for element.
 */

import type { ComponentType, SVGProps } from "react";

import {
  IconArchitecture,
  IconChat,
  IconConfig,
  IconCoverage,
  IconDashboard,
  IconFiles,
  IconGaps,
  IconGraph,
  IconHealth,
  IconStatistics,
  IconWiki,
  IconWorkspace,
} from "../components/icons.tsx";
import {
  NAV_GROUPS,
  NAV_SCOPE_LABELS,
  NAV_SCOPES,
  navItemMatches,
  navItemsFor,
  type NavItem,
} from "../nav.ts";
import { navigate } from "../router.tsx";
import { useStatisticsAwaiting } from "../views/statistics/useStatisticsAvailability.ts";
import { useWorkspace } from "../workspace/WorkspaceContext.tsx";
import { MemberSelector } from "./MemberSelector.tsx";
import styles from "./Sidebar.module.css";

/** The idiomatic icon for each nav id (keyed to nav.ts ids). */
const ICONS: Record<string, ComponentType<SVGProps<SVGSVGElement>>> = {
  overview: IconDashboard,
  health: IconHealth,
  graph: IconGraph,
  chat: IconChat,
  wiki: IconWiki,
  architecture: IconArchitecture,
  files: IconFiles,
  gaps: IconGaps,
  coverage: IconCoverage,
  statistics: IconStatistics,
  config: IconConfig,
  // Workspace mode only (S-250) — absent from a single-root sidebar.
  workspace: IconWorkspace,
  // S-428: the app-level twins of the Dashboard and Health tabs (FR-UI-36). They
  // carry their member-scoped twin's icon deliberately — the icon says WHAT the
  // view answers, and the section label says what it answers FOR (ADR-66 §4).
  "workspace-dashboard": IconDashboard,
  "workspace-health": IconHealth,
  // S-429: same reasoning — the app-level Statistics view answers the same question
  // as the member-scoped one, one scope up, so it carries the same icon (FR-UI-37).
  "workspace-statistics": IconStatistics,
};

function NavLink({ item, active, muted }: { item: NavItem; active: boolean; muted?: boolean }) {
  const current = active ? "page" : undefined;
  const Icon = ICONS[item.id];
  const content = (
    <>
      <span className={styles.icon}>{Icon && <Icon />}</span>
      <span>{item.label}</span>
    </>
  );

  const cls = [styles.item, active ? styles.active : "", muted ? styles.muted : ""]
    .filter(Boolean)
    .join(" ");
  return (
    <li className={cls}>
      <a
        className={styles.link}
        href={item.path}
        aria-current={current}
        // The muted state is advisory ("awaiting data"), never a hard disable — the
        // tab stays reachable so the user can open it and read the empty state.
        title={muted ? "Awaiting data — no telemetry recorded yet" : undefined}
        onClick={(e) => {
          e.preventDefault();
          navigate(item.path);
        }}
      >
        {content}
      </a>
    </li>
  );
}

/**
 * The CR-042 groups of `items`, in group order — the sidebar's whole body in
 * single-root mode, and the body of each scope section in workspace mode.
 *
 * A group no item falls into renders nothing rather than an empty `<ul>`: a
 * bordered empty strip under a section header is chrome for a group that is not
 * there. In single-root mode every group is non-empty, so the rendered tree is
 * element-for-element the pre-S-425 one.
 */
function NavGroups({
  items,
  pathname,
  statisticsAwaiting,
}: {
  items: readonly NavItem[];
  pathname: string;
  statisticsAwaiting: boolean;
}) {
  return (
    <>
      {NAV_GROUPS.map((group) => {
        const inGroup = items.filter((v) => v.group === group);
        if (inGroup.length === 0) return null;
        return (
          <ul className={styles.group} key={group}>
            {inGroup.map((v) => (
              <NavLink
                key={v.id}
                item={v}
                // Exact match, or — for a tab that owns client sub-routes
                // (the Wiki reader's `/wiki/page/*`) — any path under it, so the
                // tab stays highlighted while reading one of its pages. The rule
                // (and the `/`-depth guard that keeps the root Dashboard from
                // prefix-matching `/health`) lives in `nav.ts`, which is also what
                // `scopeForPath` asks.
                active={navItemMatches(v, pathname)}
                // Exact id equality, and the MEMBER-scoped tab only. S-429 added an
                // app-scoped Statistics tab with an awaiting-data state of its own,
                // and it is deliberately never muted from here: `useStatisticsAwaiting`
                // probes `/api/v1/statistics` at the SELECTED MEMBER's scope, so
                // muting the workspace tab from it would assert one member's emptiness
                // about the whole workspace. Muting it honestly would need a second,
                // unscoped fan-out read on every shell mount (NFR-PE-10). A prefix or
                // label match here would have introduced exactly that untruth.
                muted={v.id === "statistics" && statisticsAwaiting}
              />
            ))}
          </ul>
        );
      })}
    </>
  );
}

export function Sidebar({ pathname }: { pathname: string }) {
  // The Statistics item is muted when the telemetry store is empty (NFR-CC-04) —
  // an honest "awaiting data" signal that agrees with the tab's own empty state.
  const statisticsAwaiting = useStatisticsAwaiting();
  // The workspace-only tabs exist only in workspace mode (S-250, FR-UI-29; S-428
  // added two beside the original); a plain repo renders the unchanged item list.
  const { mode } = useWorkspace();
  const isWorkspace = mode === "workspace";
  const items = navItemsFor(isWorkspace);

  if (!isWorkspace) {
    return (
      <nav className={styles.sidebar} aria-label="Views">
        <NavGroups items={items} pathname={pathname} statisticsAwaiting={statisticsAwaiting} />
      </nav>
    );
  }

  return (
    <nav className={styles.sidebar} aria-label="Views">
      {NAV_SCOPES.map((scope) => {
        const scoped = items.filter((v) => v.scope === scope);
        if (scoped.length === 0) return null;
        const headingId = `nav-scope-${scope}`;
        return (
          <section className={styles.section} key={scope} aria-labelledby={headingId}>
            <div className={styles.sectionHeader}>
              <h2 className={styles.sectionLabel} id={headingId}>
                {NAV_SCOPE_LABELS[scope]}
              </h2>
              {/* The selector governs the member-scoped views and only those, so it
                  is rendered on THIS header row and nowhere else in the tree — the
                  presentation defect FR-UI-35 exists to close (NFR-CC-04). The
                  heading beside it is its accessible name; it carries no label of
                  its own (frontend-design §3: `SERVICE [ orders ▾ ]`). */}
              {scope === "member" && <MemberSelector labelledBy={headingId} />}
            </div>
            <NavGroups items={scoped} pathname={pathname} statisticsAwaiting={statisticsAwaiting} />
          </section>
        );
      })}
    </nav>
  );
}
