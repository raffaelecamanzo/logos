import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { ScoreBar } from "./ScoreBar.tsx";

afterEach(() => cleanup());

describe("ScoreBar (S-187, FR-UI-23 / ADR-44)", () => {
  it("is a native <meter> with the value attribute, not an inline style (CSP-safe)", () => {
    const { container } = render(<ScoreBar value={8500} max={10_000} label="85.0%" />);
    const meter = container.querySelector("meter");
    expect(meter).not.toBeNull();
    expect(meter?.getAttribute("value")).toBe("8500");
    expect(meter?.getAttribute("max")).toBe("10000");
    expect(meter?.getAttribute("style")).toBeNull();
    // The label is the no-JS / SR fallback text.
    expect(meter?.textContent).toBe("85.0%");
  });
  it("clamps an out-of-range value rather than overflowing the track", () => {
    const { container } = render(<ScoreBar value={20_000} max={10_000} label="x" />);
    expect(container.querySelector("meter")?.getAttribute("value")).toBe("10000");
  });
  it("clamps a negative value to zero", () => {
    const { container } = render(<ScoreBar value={-5} max={10_000} label="x" />);
    expect(container.querySelector("meter")?.getAttribute("value")).toBe("0");
  });

  it("maps each tone to a distinct fill class (the band/magnitude tints, hash-agnostic)", () => {
    const cls = (tone: Parameters<typeof ScoreBar>[0]["tone"]) => {
      const { container } = render(<ScoreBar value={5_000} tone={tone} label="x" />);
      const name = container.querySelector("meter")?.className ?? "";
      cleanup();
      return name;
    };
    const tones = ["default", "poor", "average", "good", "excellent", "magnitude"] as const;
    const classes = tones.map(cls);
    // Every tone yields a non-empty class, and the six are mutually distinct (so the
    // BR-34 bands and the neutral magnitude fill never collapse to one colour).
    expect(classes.every((c) => c.length > 0)).toBe(true);
    expect(new Set(classes).size).toBe(tones.length);
  });
});
