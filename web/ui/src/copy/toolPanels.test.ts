// The tool-panel register (S-617, CR-203 §3.2 A, FR-UI-39): every panel is named,
// says in one line what it is for, and says why it is exempt from why/action/where.
// Which views render each key is the widget source scan's half
// (`views/widgetScan.test.ts`).
import { describe, expect, it } from "vitest";

import { copyTextString, findUnglossedTerms } from "./text.ts";
import { isToolPanelKey, TOOL_PANELS } from "./toolPanels.ts";

const panels = Object.entries(TOOL_PANELS);

describe("TOOL_PANELS", () => {
  it.each(panels)("%s is named, with one line of what and a reason", (_key, panel) => {
    expect(panel.name.trim()).not.toBe("");
    expect(panel.reason.trim()).not.toBe("");
    const what = copyTextString(panel.what);
    // One line: one sentence, ending in a full stop.
    expect(what).toMatch(/^[A-Z].*\.$/);
    expect(what.slice(0, -1)).not.toMatch(/\.\s/);
  });

  it.each(panels)("%s glosses every internal term in its line", (_key, panel) => {
    expect(findUnglossedTerms(panel.what)).toEqual([]);
  });

  it("looks a key up as an own key only", () => {
    expect(isToolPanelKey("graphQuery")).toBe(true);
    expect(isToolPanelKey("constructor")).toBe(false);
    expect(isToolPanelKey("toString")).toBe(false);
  });
});
