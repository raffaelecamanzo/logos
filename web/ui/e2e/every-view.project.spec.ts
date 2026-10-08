// The widget layout standard on EVERY project view (S-617, CR-203 §6, FR-UI-40):
// the S-613 computed-style check — one equal gap between consecutive widgets,
// every part left-aligned, explanation and action in one body size — over each
// view the single-repository sidebar offers, read in a real browser from the
// served bundle.
//
// "Every" is the sidebar's own list (`NAV_ITEMS` in src/nav.ts), not a hand-kept
// one: the first test fails until a new view has a case here.
import { expect, test, type Page } from "@playwright/test";

import { NAV_ITEMS } from "../src/nav.ts";
import { expectWidgetStackLayout } from "./layout.ts";
import { configuredChat, layoutCase, type ViewCase } from "./views.ts";

const CASES: Record<string, ViewCase> = {
  "/": { ready: "Code coverage" },
  "/health": { ready: "Gate" },
  "/graph": { ready: "Query the whole graph" },
  // The fixture declares no chat model or key, which renders the configure-first
  // advisory and no widget at all; the configured chat is served instead.
  "/chat": { ready: "Conversation", setup: (page: Page) => configuredChat(page, "/api/v1/config") },
  "/wiki": { ready: "Welcome to the wiki" },
  // The cycle list is hidden (S-612): the matrix is the view's one widget.
  "/architecture": { ready: "Dependency matrix", single: true },
  "/files": { ready: "Files ranked by risk" },
  "/gaps": { ready: "Rule findings" },
  "/coverage": { ready: "Per-file coverage" },
  "/statistics": { ready: "Tool attribution by class" },
  "/config": { ready: "rules.toml" },
};

test("a layout case exists for every view the project sidebar offers", () => {
  expect(Object.keys(CASES).sort()).toEqual(NAV_ITEMS.map((item) => item.path).sort());
});

for (const [path, view] of Object.entries(CASES)) {
  test(`${path} keeps the widget layout standard`, async ({ page }) => {
    const { stack, titles } = await layoutCase(page, path, view);
    await expectWidgetStackLayout(stack, { single: view.single });
    test.info().annotations.push({ type: "widgets", description: `${path}: ${titles.join(" | ")}` });
  });
}
