// The harness serves the real app (S-611): a tree-built `logos serve --ui` over
// the single-repository fixture renders the shell, under its own CSP.
import { expect, test } from "@playwright/test";

import { composesNoExtension } from "./views.ts";

test("the project fixture serves the app shell as a single repository", async ({ page, request }) => {
  // Not a workspace: the cross-service surface answers only under a manifest.
  expect((await request.get("/api/v1/workspace/status")).ok()).toBe(false);

  await page.goto("/");
  await expect(page.getByRole("navigation", { name: "Views" })).toBeVisible();
  await expect(page.locator("main#view-root")).toBeVisible();
});

// The sidebar leads with the value proposition (S-788, CR-244, FR-UI-55): assess,
// then explore, then configure. Architecture is hidden through the register, so it is
// not rendered; a single repository renders no section label, selector or workspace
// entry.
test("the project sidebar renders Dashboard, Quality, Security, Coverage | Graph, Rule findings, Files & Risk | Statistics, Config", async ({ page, request }) => {
  test.skip(!(await composesNoExtension(request)), "the served build composes an extension");
  await page.goto("/");
  const nav = page.getByRole("navigation", { name: "Views" });
  await expect(nav).toBeVisible();
  const groups = nav.locator("ul");
  await expect(groups).toHaveCount(3);
  expect(await groups.evaluateAll((uls) => uls.map((ul) => [...ul.querySelectorAll("a")].map((a) => a.textContent?.trim())))).toEqual([
    ["Dashboard", "Quality", "Security", "Coverage"],
    ["Graph", "Rule findings", "Files & Risk"],
    ["Statistics", "Config"],
  ]);
  await expect(nav.getByRole("heading")).toHaveCount(0);
  await expect(nav.getByRole("combobox")).toHaveCount(0);
  await expect(nav.locator('a[href^="/workspace"]')).toHaveCount(0);
});
