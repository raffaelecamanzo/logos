import { describe, expect, it } from "vitest";

import { removeHiddenWidgetEntry } from "../test/hiddenWidgets.ts";
import { HIDDEN_WIDGETS, isWidgetHidden } from "./hiddenWidgets.ts";

describe("the hidden-widget register (S-612, FR-UI-41)", () => {
  it("hides exactly the four CR-203 widgets", () => {
    expect(HIDDEN_WIDGETS.map((w) => w.id)).toEqual([
      "coverage-by-relation-arm",
      "cross-service-impact",
      "non-gated-tier",
      "architecture-cycles",
    ]);
  });

  it("names, for every entry, the widget, where it was, why, and what still serves its data", () => {
    for (const w of HIDDEN_WIDGETS) {
      expect(w.widget.trim()).not.toBe("");
      expect(w.hiddenFrom.trim()).not.toBe("");
      expect(w.reason.trim()).not.toBe("");
      expect(w.stillServedBy.length).toBeGreaterThan(0);
      for (const surface of w.stillServedBy) expect(surface.trim()).not.toBe("");
    }
  });

  it("answers from the register, so removing an entry un-hides that widget alone", () => {
    expect(isWidgetHidden("non-gated-tier")).toBe(true);
    const restore = removeHiddenWidgetEntry("non-gated-tier");
    try {
      expect(isWidgetHidden("non-gated-tier")).toBe(false);
      expect(isWidgetHidden("architecture-cycles")).toBe(true);
    } finally {
      restore();
    }
    expect(isWidgetHidden("non-gated-tier")).toBe(true);
    expect(HIDDEN_WIDGETS.map((w) => w.id)[2]).toBe("non-gated-tier");
  });

  it("restores the declared order whichever removal is restored first", () => {
    // vitest runs `afterEach` before `onTestFinished`, so a test that removes one
    // entry inside a block that removed another restores them first-in-first-out.
    const declared = HIDDEN_WIDGETS.map((w) => w.id);
    const restoreImpact = removeHiddenWidgetEntry("cross-service-impact");
    const restoreArms = removeHiddenWidgetEntry("coverage-by-relation-arm");
    restoreImpact();
    restoreArms();
    expect(HIDDEN_WIDGETS.map((w) => w.id)).toEqual(declared);
  });

  it("refuses to remove an entry that is not in the register, rather than another one", () => {
    // Without the guard `findIndex` gives -1 and `splice(-1, 1)` would silently
    // drop the LAST entry — a return test would then run against a register
    // missing a widget it never named.
    const restore = removeHiddenWidgetEntry("non-gated-tier");
    try {
      expect(() => removeHiddenWidgetEntry("non-gated-tier")).toThrow(
        /not in the hidden-widget register/,
      );
      expect(isWidgetHidden("architecture-cycles")).toBe(true);
    } finally {
      restore();
    }
  });
});
