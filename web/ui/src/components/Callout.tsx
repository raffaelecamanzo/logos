/*
 * Callout / Verdict band (S-193, FR-UI-23; frontend-design §1.1/§5). The
 * verdict-first element every view leads with: a full-width band with an uppercase
 * tracked label and a 4px LEFT accent border in the tone colour — the brand's
 * callout language. Tones:
 *   - signal — red    (GATE/FAIL, STALE, threshold breach — the default)
 *   - warm   — orange (in-flight / heuristic / NOTE)
 *   - pass   — green  (PASS / healthy)
 *   - muted  — neutral(NON-GATED TIER / informational)
 * Red stays signal-only: the band is a thin left edge + tint, never a large fill.
 */

import type { ReactNode } from "react";

import styles from "./Callout.module.css";

export type CalloutTone = "signal" | "warm" | "pass" | "muted";

export interface CalloutProps {
  /** The uppercase tracked label (GATE, HOTSPOT, STALE, NOTE, …). */
  label: string;
  tone?: CalloutTone;
  children: ReactNode;
  className?: string;
}

export function Callout({ label, tone = "signal", children, className }: CalloutProps) {
  const cls = [styles.callout, styles[tone], className].filter(Boolean).join(" ");
  return (
    <section className={cls} role="status">
      <span className={styles.label}>{label}</span>
      <div className={styles.body}>{children}</div>
    </section>
  );
}
