/*
 * The OWASP Top 10 edition switch (FR-SC-04): two toggle buttons, 2025 then 2021.
 * Shared by the member Security page (S-670) and the Workspace security roll-up
 * (S-674), so both offer the edition the same way. Switching reads the other
 * edition from a payload that carries both — it never refetches.
 */

import type { OwaspEditionYear } from "../../api/types.ts";
import { Button } from "../../components/index.ts";
import { EDITIONS } from "./securityModel.ts";
import styles from "./SecurityView.module.css";

export function EditionSwitch({
  edition,
  onChange,
}: {
  edition: OwaspEditionYear;
  onChange: (year: OwaspEditionYear) => void;
}) {
  return (
    <div className={styles.switch} role="group" aria-label="OWASP edition">
      {EDITIONS.map((year) => (
        <Button
          key={year}
          size="sm"
          variant={year === edition ? "primary" : "secondary"}
          aria-pressed={year === edition}
          onClick={() => onChange(year)}
        >
          {year}
        </Button>
      ))}
    </div>
  );
}
