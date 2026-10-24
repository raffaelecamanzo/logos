/*
 * SeverityRow (S-789, FR-UI-09) — the fresh, unsuppressed findings of one
 * project by severity, one badge per severity in the order the Security page
 * sorts them: critical, high, medium, unrated, low, info. The Dashboard's
 * Security widget renders it, and the Workspace Dashboard's per-member rows
 * reuse it (S-790), so a severity is spelled one way wherever it is counted.
 *
 * Every figure is the read-model's own count, shown with its word — a severity
 * with none reads `0`, muted, never left out, so a row states every severity it
 * looked for. `severityTone` colours the badge and the word always rides with
 * it (a11y). Presentation only: it writes nothing and reads nothing.
 */

import { Badge } from "../../components/index.ts";
import { fmtInt } from "../dashboard/dashboardModel.ts";
import { SEVERITIES, severityTone, type SeverityCounts } from "./securityModel.ts";
import styles from "./SeverityRow.module.css";

export function SeverityRow({ counts }: { counts: SeverityCounts }) {
  return (
    <ul className={styles.row} aria-label="Findings by severity" data-severity-row="">
      {SEVERITIES.map((severity) => (
        <li key={severity} className={styles.item} data-severity={severity}>
          <Badge tone={counts[severity] === 0 ? "muted" : severityTone(severity)}>{severity}</Badge>
          <span className="mono num">{fmtInt(counts[severity])}</span>
        </li>
      ))}
    </ul>
  );
}
