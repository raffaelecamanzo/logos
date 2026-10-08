// The harness serves the real app (S-611): a tree-built `logos serve --ui` over
// the single-repository fixture renders the shell, under its own CSP.
import { expect, test } from "@playwright/test";

test("the project fixture serves the app shell as a single repository", async ({ page, request }) => {
  // Not a workspace: the cross-service surface answers only under a manifest.
  expect((await request.get("/api/v1/workspace/status")).ok()).toBe(false);

  await page.goto("/");
  await expect(page.getByRole("navigation", { name: "Views" })).toBeVisible();
  await expect(page.locator("main#view-root")).toBeVisible();
});
