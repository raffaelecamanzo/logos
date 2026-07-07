/*
 * Tabs (S-193, FR-UI-23; WAI-ARIA tabs pattern). An accessible tab set:
 * role=tablist/tab/tabpanel, `aria-selected`, roving tabindex, and arrow-key
 * navigation (Left/Right/Home/End) with the focused tab activated. The global
 * :focus-visible ring marks the focused tab.
 */

import { useId, useRef, useState } from "react";
import type { ReactNode } from "react";

import styles from "./Tabs.module.css";

export interface TabItem {
  id: string;
  label: ReactNode;
  panel: ReactNode;
}

export interface TabsProps {
  tabs: TabItem[];
  /** Initially-selected tab id (defaults to the first). */
  defaultTabId?: string;
  /** Accessible name for the tablist. */
  label: string;
  className?: string;
}

export function Tabs({ tabs, defaultTabId, label, className }: TabsProps) {
  const [selected, setSelected] = useState<string>(defaultTabId ?? tabs[0]?.id ?? "");
  const baseId = useId();
  const tabRefs = useRef<Record<string, HTMLButtonElement | null>>({});

  function focusTab(id: string) {
    setSelected(id);
    tabRefs.current[id]?.focus();
  }

  function onKeyDown(e: React.KeyboardEvent, index: number) {
    const last = tabs.length - 1;
    let next: number | null = null;
    if (e.key === "ArrowRight") next = index === last ? 0 : index + 1;
    else if (e.key === "ArrowLeft") next = index === 0 ? last : index - 1;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = last;
    if (next !== null) {
      e.preventDefault();
      focusTab(tabs[next].id);
    }
  }

  return (
    <div className={[styles.tabs, className].filter(Boolean).join(" ")}>
      <div role="tablist" aria-label={label} className={styles.tablist}>
        {tabs.map((tab, i) => {
          const active = tab.id === selected;
          return (
            <button
              key={tab.id}
              ref={(el) => {
                tabRefs.current[tab.id] = el;
              }}
              type="button"
              role="tab"
              id={`${baseId}-tab-${tab.id}`}
              aria-selected={active}
              aria-controls={`${baseId}-panel-${tab.id}`}
              tabIndex={active ? 0 : -1}
              className={active ? `${styles.tab} ${styles.active}` : styles.tab}
              onClick={() => setSelected(tab.id)}
              onKeyDown={(e) => onKeyDown(e, i)}
            >
              {tab.label}
            </button>
          );
        })}
      </div>
      {tabs.map((tab) => (
        <div
          key={tab.id}
          role="tabpanel"
          id={`${baseId}-panel-${tab.id}`}
          aria-labelledby={`${baseId}-tab-${tab.id}`}
          hidden={tab.id !== selected}
          tabIndex={0}
          className={styles.panel}
        >
          {tab.panel}
        </div>
      ))}
    </div>
  );
}
