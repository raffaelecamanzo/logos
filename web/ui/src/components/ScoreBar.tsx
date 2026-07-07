/*
 * ScoreBar (S-187, FR-UI-09/FR-UI-04, FR-UI-23; ADR-44). The shared 0–max score
 * bar the Dashboard roll-ups and the Health metric grid render — the React twin of
 * the server-rendered `score_bar` (web/src/components.rs). A native `<meter>` whose
 * fill is the `value` attribute (CSP-clean — no inline style, ADR-44); the `label`
 * is the no-JS/SR fallback text. An out-of-range value clamps rather than
 * overflowing the track (mirrors the Rust helper).
 *
 * Tone is a fixed enumeration so each tint is its own CSS-module class (no inline
 * colour): `default` green, the four BR-34 quality bands (poor → excellent), and
 * `magnitude` (a neutral fill for a raw count — never read as a "pass" green).
 * Coverage/test bars stay `default` green and raw (never banded — BR-28).
 */

import styles from "./ScoreBar.module.css";

/** The score-bar fill tone (BR-34 bands + neutral magnitude). */
export type ScoreBarTone = "default" | "poor" | "average" | "good" | "excellent" | "magnitude";

export interface ScoreBarProps {
  /** The figure to plot; clamped to `[0, max]`. */
  value: number;
  /** The track maximum (default 10000 — the 0–10000 quality scale). */
  max?: number;
  /** The fill tone (default green; bands tint only the quality bar). */
  tone?: ScoreBarTone;
  /** The no-JS / screen-reader fallback text (e.g. "85.0%" or "8500 / 10000"). */
  label: string;
}

export function ScoreBar({ value, max = 10_000, tone = "default", label }: ScoreBarProps) {
  const ceiling = Math.max(1, max);
  const clamped = Math.min(ceiling, Math.max(0, value));
  const cls = [styles.bar, styles[tone]].filter(Boolean).join(" ");
  return (
    <meter className={cls} min={0} max={ceiling} value={clamped}>
      {label}
    </meter>
  );
}
