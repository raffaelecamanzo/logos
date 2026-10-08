// The widget frame in a real browser (S-611, FR-UI-40): equal gaps between
// consecutive widgets, left-aligned widget text, one body size for explanation
// and action — read as COMPUTED style from the real components and stylesheets.
//
// No view renders a Widget yet, so the page is the harness build (global-setup.ts)
// served at the logos origin under the same self-only CSP the SPA gets.
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

import { expect, test } from "@playwright/test";

import { expectWidgetStackLayout, measureStack } from "./layout.ts";

const HARNESS_DIST = fileURLToPath(new URL("./.harness-dist", import.meta.url));
const TYPES: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
};

test.beforeEach(async ({ page }) => {
  await page.route("**/__e2e/harness/**", async (route) => {
    const rel = new URL(route.request().url()).pathname.replace(/^\/__e2e\/harness\/?/, "") || "index.html";
    const file = normalize(join(HARNESS_DIST, rel));
    if (!file.startsWith(HARNESS_DIST)) return route.fulfill({ status: 404 });
    const body = await readFile(file).catch(() => null);
    if (body === null) return route.fulfill({ status: 404 });
    await route.fulfill({
      status: 200,
      body,
      headers: {
        "content-type": TYPES[extname(file)] ?? "application/octet-stream",
        // The served SPA's policy: no inline script or style may run the frame.
        "content-security-policy": "default-src 'self'",
      },
    });
  });
});

async function openHarness(page: import("@playwright/test").Page) {
  const violations: string[] = [];
  page.on("console", (msg) => {
    if (/Content Security Policy/i.test(msg.text())) violations.push(msg.text());
  });
  await page.goto("/__e2e/harness/index.html");
  const stack = page.locator("[data-widget-stack]");
  await expect(stack.locator("[data-widget]")).toHaveCount(4);
  return { stack, violations };
}

test("consecutive widgets share one gap, widget text is left-aligned, explanation and action share one size", async ({
  page,
}) => {
  const { stack, violations } = await openHarness(page);
  const m = await expectWidgetStackLayout(stack);
  expect(m.gaps).toHaveLength(3);
  // The gap is the --space-5 token (1.5rem at the 16px root).
  expect(m.rowGap).toBe(24);
  expect(violations, "the frame needs no CSP exception").toEqual([]);
});

test("an absence is a left-aligned statement in the figure row, not a centred empty state", async ({ page }) => {
  const { stack } = await openHarness(page);
  const absence = stack.locator("[data-widget-absence]");
  await expect(absence).toHaveText("No coverage ingested yet.");
  await expect(absence).toHaveCSS("text-align", /^(start|left)$/);
  // The statement sits IN the figure row, starting at the row's left edge: a
  // centred row (`justify-content: center` on the flex row) would move it.
  await expect(stack.locator('[data-widget-part="figure"] > [data-widget-absence]')).toHaveCount(1);
  const [row, statement] = await Promise.all([
    stack.locator('[data-widget-part="figure"]:has(> [data-widget-absence])').boundingBox(),
    absence.boundingBox(),
  ]);
  expect(statement!.x).toBeCloseTo(row!.x, 0);
});

test("the layout check catches one widget whose margin changed", async ({ page }) => {
  const { stack } = await openHarness(page);
  // The mutation: one widget carries its own margin, as a stray stylesheet rule would.
  await stack.locator(":scope > *").nth(2).evaluate((el) => {
    (el as HTMLElement).style.marginTop = "8px";
  });
  const m = await measureStack(stack);
  expect(m.gaps[1]).toBeCloseTo(m.rowGap + 8, 1);
  await expect(expectWidgetStackLayout(stack)).rejects.toThrow(/gap 2 of 3/);
});

test("a glossed term shows its explanation on hover and on keyboard focus, and hides it at rest", async ({
  page,
}) => {
  const { stack } = await openHarness(page);
  const term = stack.locator('dfn[data-term="arm"]');
  const tip = term.getByRole("tooltip");
  await expect(tip).toBeHidden();

  await term.hover();
  await expect(tip).toBeVisible();
  await page.mouse.move(0, 0);
  await expect(tip).toBeHidden();

  // The keyboard path is why Term is not a plain `title` attribute.
  await page.keyboard.press("Tab");
  await expect(term).toBeFocused();
  await expect(tip).toBeVisible();
});
