/*
 * Badge (S-193, FR-UI-23; frontend-design §5). A small uppercase status chip that
 * ALWAYS carries text — colour is never the only signal (WCAG 2.1 AA, §7). Tones:
 *   - red    — fail / error / stale     (white ink on signal-red, 4.87:1)
 *   - orange — warning / in-flight      (near-black ink on warm, 4.65:1)
 *   - green  — pass / bound / covered   (near-black ink on green, 5.7:1)
 *   - muted  — n/a / info               (outline; muted text on surface, ≥4.5:1)
 * The solid hues are identical in both themes (they are signals), so the ink is too.
 */

import type { ReactNode } from "react";

import styles from "./Badge.module.css";

export type BadgeTone = "red" | "orange" | "green" | "muted";

export interface BadgeProps {
  tone: BadgeTone;
  children: ReactNode;
  className?: string;
}

export function Badge({ tone, children, className }: BadgeProps) {
  const cls = [styles.badge, styles[tone], className].filter(Boolean).join(" ");
  return <span className={cls}>{children}</span>;
}
