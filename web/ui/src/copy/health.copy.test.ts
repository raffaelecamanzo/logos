// The Health catalogue's figure helpers (S-615): the edges a rendered fixture
// does not reach.
import { describe, expect, it } from "vitest";

import { percent } from "./health.copy.ts";

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
