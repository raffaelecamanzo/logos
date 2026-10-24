import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { SeverityRow } from "./SeverityRow.tsx";

afterEach(cleanup);

const COUNTS = { critical: 1, high: 12, medium: 0, low: 1_234, info: 0, unrated: 2, total: 1_249 };

describe("SeverityRow (S-789, reused by S-790)", () => {
  it("lists every severity in the Security page's order, each with its own grouped count", () => {
    const { container } = render(<SeverityRow counts={COUNTS} />);
    const rows = [...container.querySelectorAll("[data-severity]")].map((li) => [
      li.getAttribute("data-severity"),
      li.querySelector(".num")?.textContent,
    ]);
    expect(rows).toEqual([
      ["critical", "1"],
      ["high", "12"],
      ["medium", "0"],
      ["unrated", "2"],
      ["low", "1,234"],
      ["info", "0"],
    ]);
  });

  it("carries the severity word with its badge, so colour is never the only cue", () => {
    render(<SeverityRow counts={COUNTS} />);
    for (const word of ["critical", "high", "medium", "unrated", "low", "info"]) {
      expect(screen.getByText(word)).toBeInTheDocument();
    }
    expect(screen.getByLabelText("Findings by severity")).toBeInTheDocument();
  });

  it("mutes a severity with no findings and tones one that has them", () => {
    const { container } = render(<SeverityRow counts={COUNTS} />);
    const badgeOf = (severity: string) =>
      container.querySelector(`[data-severity="${severity}"] span`)!.className;
    // A zero reads as the muted badge a low finding gets; a critical one does not.
    expect(badgeOf("medium")).toBe(badgeOf("info"));
    expect(badgeOf("critical")).not.toBe(badgeOf("medium"));
    expect(badgeOf("high")).toBe(badgeOf("critical"));
  });

  it("tones each severity that has findings as the Security page does: critical and high red, medium orange, the rest muted", () => {
    const all = { critical: 1, high: 1, medium: 1, low: 1, info: 1, unrated: 1, total: 6 };
    const { container } = render(<SeverityRow counts={all} />);
    const badgeOf = (severity: string) =>
      container.querySelector(`[data-severity="${severity}"] span`)!.className;
    expect(badgeOf("critical")).toBe(badgeOf("high"));
    expect(new Set([badgeOf("low"), badgeOf("info"), badgeOf("unrated")]).size).toBe(1);
    expect(new Set([badgeOf("critical"), badgeOf("medium"), badgeOf("low")]).size).toBe(3);
  });
});
