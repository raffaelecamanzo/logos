/*
 * AppShell (S-193, FR-UI-23; frontend-design §3). The structural layout primitive:
 * a fixed sidebar column + a main column (header over scrolling content), with
 * semantic landmarks and a keyboard skip-link to the content. View-agnostic — the
 * shell and every migrated view compose into its `sidebar` / `header` / children
 * slots. Below the tablet breakpoint the sidebar collapses above the content.
 */

import type { ReactNode } from "react";

import styles from "./AppShell.module.css";

export interface AppShellProps {
  sidebar: ReactNode;
  header: ReactNode;
  children: ReactNode;
}

export function AppShell({ sidebar, header, children }: AppShellProps) {
  return (
    <div className={styles.app}>
      <a className={styles.skipLink} href="#view-root">
        Skip to content
      </a>
      {sidebar}
      <div className={styles.main}>
        {header}
        <main className={styles.content} id="view-root" tabIndex={-1}>
          {children}
        </main>
      </div>
    </div>
  );
}
