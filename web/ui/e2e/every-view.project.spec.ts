// The widget layout standard on EVERY project view (S-617, CR-203 §6, FR-UI-40):
// the S-613 computed-style check — one equal gap between consecutive widgets,
// every part left-aligned, the explanation at the body size, no action line
// (CR-206) — over each view the single-repository sidebar offers, read in a real
// browser from the served bundle.
//
// "Every" is the sidebar's own list (`navItemsFor` in src/nav.ts — the registry
// less what the hidden-widget register hides), not a hand-kept one: the first
// test fails until a new view has a case here.
import { expect, test, type Page } from "@playwright/test";

import { navItemsFor } from "../src/nav.ts";
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
  "/files": { ready: "Files ranked by risk" },
  "/gaps": { ready: "Rule findings" },
  "/coverage": { ready: "Per-file coverage" },
  "/statistics": { ready: "Tool attribution by class" },
  "/config": { ready: "rules.toml" },
};

test("a layout case exists for every view the project sidebar offers", () => {
  expect(Object.keys(CASES).sort()).toEqual(navItemsFor(false).map((item) => item.path).sort());
});

// The Architecture view is hidden through the register (CR-208, FR-UI-41): the
// sidebar does not offer it, and its route and the retired `/dsm` land on Health.
for (const path of ["/architecture", "/dsm"]) {
  test(`${path} lands on Health, and the sidebar offers no Architecture entry`, async ({ page }) => {
    await page.goto(path);
    await expect(page).toHaveURL(/\/health$/);
    await expect(page.locator("main#view-root").getByRole("heading", { name: "Gate", exact: true })).toBeVisible();
    await expect(page.getByRole("navigation", { name: "Views" }).getByRole("link", { name: /Architecture/ })).toHaveCount(0);
  });
}

for (const [path, view] of Object.entries(CASES)) {
  test(`${path} keeps the widget layout standard`, async ({ page }) => {
    const { stack, titles } = await layoutCase(page, path, view);
    await expectWidgetStackLayout(stack, { single: view.single });
    test.info().annotations.push({ type: "widgets", description: `${path}: ${titles.join(" | ")}` });
  });
}
