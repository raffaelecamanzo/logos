import { describe, expect, it } from "vitest";

// Vite's `?raw` loader (typed by vite/client, see src/vite-env.d.ts) hands back the
// file's literal text — the only source-reading mechanism this Vite/Vitest project
// uses, so the audit never introduces a `node:fs` dependency the SPA build doesn't
// otherwise have.
import dashboardSource from "./DashboardView.tsx?raw";
import { enumerateDashboardCards } from "./cardEnumeration.ts";

/**
 * S-438 audit, recorded 2026-09-21: every Dashboard card enumerated from source,
 * 7 of 7 conformant — RuleFindingsCard was the one corrected by this task (it
 * rendered PASS over a zero-rule contract without consulting `checked_rules`;
 * see DashboardView.test.tsx for the behavioral coverage of the fix). The other
 * six were already honest about absence and needed no change:
 *   ProjectOverviewCard, QualityCard, LanguagesCard, GraphCard, ActivityCard,
 *   CodeCoverageCard — conformant.
 *   RuleFindingsCard — corrected.
 *
 * This count is a dated finding, not a floor: a card added later legitimately
 * grows the list and must not fail this test (`arrayContaining`, not equality).
 * The second test below demonstrates the enumeration itself would still notice
 * that addition, so the audit tool is not merely reporting "looks fine".
 */
const AUDITED_CONFORMANT_OR_CORRECTED = [
  "ProjectOverviewCard",
  "QualityCard",
  "LanguagesCard",
  "GraphCard",
  "ActivityCard",
  "RuleFindingsCard",
  "CodeCoverageCard",
];

describe("Dashboard card enumeration (S-438 audit, CR-141)", () => {
  it("enumerates every Card-returning component in DashboardView.tsx — audited 2026-09-21, 7 of 7 conformant or corrected", () => {
    const cards = enumerateDashboardCards(dashboardSource);
    expect(cards.map((c) => c.component)).toEqual(expect.arrayContaining(AUDITED_CONFORMANT_OR_CORRECTED));
    expect(cards.find((c) => c.component === "RuleFindingsCard")?.title).toBe("Rule findings");
  });

  it("detects a card the last audit never saw, rather than merely running clean over the current file", () => {
    const before = enumerateDashboardCards(dashboardSource);

    const mutated = `${dashboardSource}\n\nfunction ExtraCard() {\n  return (\n    <Card title="Extra">\n      <p>unaudited</p>\n    </Card>\n  );\n}\n`;
    const after = enumerateDashboardCards(mutated);

    expect(after.length).toBe(before.length + 1);
    const added = after.find((c) => c.component === "ExtraCard");
    expect(added?.title).toBe("Extra");
  });
});
