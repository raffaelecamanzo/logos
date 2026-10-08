/*
 * WidgetStack (S-611, CR-203, FR-UI-40). The one container a view stacks its
 * widgets in: a column whose children are separated by ONE spacing token, so
 * every pair of consecutive widgets sits the same distance apart. The gap is the
 * stack's, never a child's margin — a widget that brought its own margin would
 * break the equal-gap rule the Playwright layout spec asserts.
 */

import type { ReactNode } from "react";

import styles from "./WidgetStack.module.css";

export interface WidgetStackProps {
  children: ReactNode;
  className?: string;
}

export function WidgetStack({ children, className }: WidgetStackProps) {
  const cls = [styles.stack, className].filter(Boolean).join(" ");
  return (
    <div className={cls} data-widget-stack="">
      {children}
    </div>
  );
}
