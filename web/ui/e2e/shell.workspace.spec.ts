// The harness serves the real app (S-611): a tree-built `logos serve --ui` over
// the two-member workspace fixture renders the workspace shell.
import { expect, test } from "@playwright/test";

import { composesNoExtension } from "./views.ts";

test("the workspace fixture serves the workspace shell over its two members", async ({ page, request }) => {
  // The server is in workspace mode, over exactly the fixture's members.
  const status = await request.get("/api/v1/workspace/status");
  expect(status.ok()).toBe(true);
  const members = ((await status.json()) as { members: { member: string }[] }).members.map((m) => m.member);
  expect(members.sort()).toEqual(["api", "web"]);

  await page.goto("/");
  await expect(page.getByRole("navigation", { name: "Views" })).toBeVisible();
  await expect(page.locator("main#view-root")).toBeVisible();
});

// The sidebar leads with the value proposition in both sections (S-788, CR-244,
// FR-UI-55). The Workspace section reads as one list; its Coverage entry is rendered
// only once a view owns its route, so it is absent until the roll-up view lands.
test("the workspace sidebar renders both sections in the new order", async ({ page, request }) => {
  test.skip(!(await composesNoExtension(request)), "the served build composes an extension");
  await page.goto("/");
  const nav = page.getByRole("navigation", { name: "Views" });
  await expect(nav).toBeVisible();
  const labels = (section: string) =>
    nav
      .getByRole("region", { name: section })
      .locator("a")
      .evaluateAll((links) => links.map((a) => a.textContent?.trim()));
  await expect(nav.getByRole("region", { name: "Workspace" })).toBeVisible();
  const workspace = await labels("Workspace");
  expect(workspace.slice(0, 4)).toEqual(["Dashboard", "Workspace", "Health", "Security"]);
  expect(workspace.slice(-2)).toEqual(["Statistics", "Config"]);
  // Coverage, if a view has been registered for it, sits between Security and Statistics.
  expect(workspace.filter((l) => l === "Coverage").length).toBeLessThanOrEqual(1);
  if (workspace.includes("Coverage")) expect(workspace.indexOf("Coverage")).toBe(4);
  expect(await labels("Service")).toEqual([
    "Dashboard",
    "Health",
    "Security",
    "Coverage",
    "Graph",
    "Rule findings",
    "Files & Risk",
    "Statistics",
    "Config",
  ]);
});
