// The widget layout standard on EVERY workspace view (S-617, CR-203 §6,
// FR-UI-40): the S-613 computed-style check over each app-level view the
// workspace sidebar adds, read in a real browser over the two-member fixture.
// The member views are the project spec's (`every-view.project.spec.ts`).
//
// "Every" is the sidebar's own list (`WORKSPACE_NAV_ITEMS` in src/nav.ts): the
// first test fails until a new workspace view has a case here.
import { expect, test, type Page } from "@playwright/test";

import { WORKSPACE_NAV_ITEMS } from "../src/nav.ts";
import { expectWidgetStackLayout } from "./layout.ts";
import { configuredChat, layoutCase, type ViewCase } from "./views.ts";

const CASES: Record<string, ViewCase> = {
  "/workspace-dashboard": { ready: "Members" },
  "/workspace-health": { ready: "Broker topics" },
  // The Service map tab, which the view opens on; the coverage tab is
  // `workspace-widgets.workspace.spec.ts`'s.
  "/workspace": { ready: "Cross-service bindings" },
  // The fixture declares no workspace chat tier; the configured chat is served.
  "/workspace-chat": {
    ready: "Conversation",
    setup: (page: Page) => configuredChat(page, "/api/v1/workspace/config"),
  },
  "/workspace-statistics": { ready: "Estimated value" },
  "/workspace-config": { ready: "Workspace chat policy and credential" },
};

test("a layout case exists for every view the workspace sidebar adds", () => {
  expect(Object.keys(CASES).sort()).toEqual(WORKSPACE_NAV_ITEMS.map((item) => item.path).sort());
});

for (const [path, view] of Object.entries(CASES)) {
  test(`${path} keeps the widget layout standard`, async ({ page }) => {
    const { stack, titles } = await layoutCase(page, path, view);
    await expectWidgetStackLayout(stack, { single: view.single });
    test.info().annotations.push({ type: "widgets", description: `${path}: ${titles.join(" | ")}` });
  });
}
