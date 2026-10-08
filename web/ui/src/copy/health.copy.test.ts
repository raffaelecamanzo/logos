// The Health catalogue's figure helpers (S-615): the edges a rendered fixture
// does not reach.
import { describe, expect, it } from "vitest";

import { DIMENSION_COPY, percent } from "./health.copy.ts";

describe("percent", () => {
  it.each([
    [0, "0.0%"],
    [0.0001, "<0.1%"],
    [0.0004999, "<0.1%"],
    [0.0005, "0.1%"],
    [0.2, "20.0%"],
    [0.9994, "99.9%"],
    [0.9995, ">99.9%"],
    [0.99999, ">99.9%"],
    [1, "100.0%"],
  ])("states %f as %s", (ratio, text) => {
    expect(percent(ratio)).toBe(text);
  });
});

// CR-208 (NFR-CC-04): the Architecture view is hidden, so no dimension's pointer
// may name it or link to it — the rendered view is checked in HealthView.test.tsx;
// this holds every dimension, rendered state or not.
describe("the unlisted pointers name no hidden view (CR-208)", () => {
  it.each(Object.entries(DIMENSION_COPY))("%s points at no Architecture view or matrix", (_key, entry) => {
    const pointer = entry.unlisted;
    expect(pointer?.view?.href ?? "").not.toMatch(/^\/(architecture|dsm)\b/);
    expect(`${pointer?.statement ?? ""} ${pointer?.view?.label ?? ""}`).not.toMatch(/architecture|matrix/i);
  });
});
