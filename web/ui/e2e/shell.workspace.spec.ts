// The harness serves the real app (S-611): a tree-built `logos serve --ui` over
// the two-member workspace fixture renders the workspace shell.
import { expect, test } from "@playwright/test";

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
