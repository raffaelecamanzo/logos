/*
 * The shell sidebar (S-185, re-skinned onto the design system in S-193). Renders
 * the navigable views grouped into the three sidebar groups, each with an
 * idiomatic inline-SVG icon (frontend-design §3, CR-042). Every tab is a React
 * view: clicking pushes client-side history (router `navigate`) without a full
 * reload. The active item gets the rotated card-accent grammar (3px red left bar +
 * semibold) from Sidebar.module.css; the global :focus-visible ring marks keyboard
 * focus.
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
} from "../components/icons.tsx";
import { NAV_GROUPS, NAV_ITEMS, type NavItem } from "../nav.ts";
import { navigate } from "../router.tsx";
import { useStatisticsAwaiting } from "../views/statistics/useStatisticsAvailability.ts";
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

export function Sidebar({ pathname }: { pathname: string }) {
  // The Statistics item is muted when the telemetry store is empty (NFR-CC-04) —
  // an honest "awaiting data" signal that agrees with the tab's own empty state.
  const statisticsAwaiting = useStatisticsAwaiting();
  return (
    <nav className={styles.sidebar} aria-label="Views">
      {NAV_GROUPS.map((group) => (
        <ul className={styles.group} key={group}>
          {NAV_ITEMS.filter((v) => v.group === group).map((v) => (
            <NavLink
              key={v.id}
              item={v}
              // Exact match, or — for a tab that owns client sub-routes
              // (the Wiki reader's `/wiki/page/*`) — any path under it, so the
              // tab stays highlighted while reading one of its pages.
              // The prefix check is guarded to `/`-depth routes only: the root
              // Dashboard (`path: "/"`) must never prefix-match `/health` etc.
              active={pathname === v.path || (v.path !== "/" && pathname.startsWith(`${v.path}/`))}
              muted={v.id === "statistics" && statisticsAwaiting}
            />
          ))}
        </ul>
      ))}
    </nav>
  );
}
