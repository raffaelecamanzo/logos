// The chat prose scale and the calmer red accent, read in a real browser (S-618,
// CR-204 L1/L7/L8, FR-UI-45): computed styles in BOTH themes, from the bundle a
// tree-built `logos serve --ui` serves under its own self-only CSP.
//
// vitest cannot read these — its CSS modules are a proxy under `css: false` — and
// the Rust `spa_design_system` suite reads the stylesheet TEXT. This is the one
// place the cascade's result is observed. The turn is mocked (no outbound call, no
// provider): the config read is served as configured, and the turn's SSE is a fixed
// final answer.
import { expect, test, type Locator, type Page } from "@playwright/test";

import { configuredChat } from "./views.ts";

const ANSWER = [
  "## Second-level heading",
  "",
  "A paragraph of answer prose that is long enough to be a real line of text.",
  "",
  "### Third-level heading",
  "",
  "- a list item",
].join("\n");

// `--so-red`, as the browser serializes it.
const SO_RED = "rgb(218, 41, 28)";

// The turn's SSE: optional Activity steps ahead of the final answer.
function turnBody(withStep: boolean): string {
  const step = withStep
    ? `event: step_started\ndata: ${JSON.stringify({ index: 0, role: "graph_navigator", instruction: "map callers" })}\n\n` +
      `event: step_observed\ndata: ${JSON.stringify({ index: 0, role: "graph_navigator", summary: "## Step heading\n\nStep result prose." })}\n\n`
    : "";
  return `${step}event: final_answer\ndata: ${JSON.stringify({ answer: ANSWER })}\n\n`;
}

async function openConsentedChat(page: Page, theme: "dark" | "light", body = turnBody(false)): Promise<void> {
  await configuredChat(page, "/api/v1/config");
  await page.route("**/chat", async (route) => {
    if (route.request().method() !== "POST") return route.fallback();
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body,
    });
  });
  await page.goto("/chat");
  // An explicit theme is exactly what the persisted choice sets; the tokens key on it.
  await page.evaluate((t) => document.documentElement.setAttribute("data-theme", t), theme);
}

for (const theme of ["dark", "light"] as const) {
  test.describe(`${theme} theme`, () => {
    test("the chat tokens resolve on the document root", async ({ page }) => {
      await openConsentedChat(page, theme);
      const tokens = await page.evaluate(() => {
        const cs = getComputedStyle(document.documentElement);
        return ["--radius-3", "--measure-prose", "--text-chat", "--leading-chat", "--tracking-heading", "--elevation-composer"].map(
          (name) => [name, cs.getPropertyValue(name).trim()],
        );
      });
      // The served stylesheet is minified (`-0.025em` → `-.025em`), so compare the
      // numerically-normalised spelling on both sides.
      const norm = (v: string) => v.replace(/(^|[^\d])0\./g, "$1.");
      const byName = Object.fromEntries(tokens.map(([name, value]) => [name, norm(value)]));
      expect(byName["--radius-3"]).toBe("24px");
      expect(byName["--measure-prose"]).toBe("72ch");
      expect(byName["--text-chat"]).toBe("1rem");
      expect(byName["--leading-chat"]).toBe("1.75");
      expect(byName["--tracking-heading"]).toBe(norm("-0.025em"));
      // The composer's lift is themed: a warm-neutral shadow on light, a deep one on dark.
      expect(byName["--elevation-composer"]).toBe(
        norm(theme === "dark" ? "0 4px 16px rgba(0, 0, 0, 0.45)" : "0 4px 16px rgba(61, 57, 53, 0.12)"),
      );
    });

    test("'Start chatting' and an empty-input Send are neutral; a sendable Send is a small red circle", async ({ page }) => {
      await openConsentedChat(page, theme);
      const start = page.getByRole("button", { name: "Start chatting" });
      await expect(start).toBeVisible();
      expect(await start.evaluate((el) => getComputedStyle(el).backgroundColor)).not.toBe(SO_RED);
      await start.click();

      const input = page.getByRole("textbox", { name: "Your message" });
      const send = page.getByRole("button", { name: "Send" });
      await expect(input).toBeEnabled();
      // Empty: disabled, neutral, and no dot.
      await expect(send).toBeDisabled();
      expect(await send.evaluate((el) => getComputedStyle(el).backgroundColor)).not.toBe(SO_RED);
      expect(await send.evaluate((el) => getComputedStyle(el, "::before").content)).toBe("none");

      // Sendable: the round in-pill Send (S-619) is the signal itself — red, and small
      // enough (≤ 40×40px) to carry it. S-618's dot is gone: one sendable signal.
      await input.fill("what changed?");
      await expect(send).toBeEnabled();
      // The fill eases in over `--motion-fast`: wait for it to settle before reading.
      await expect.poll(() => send.evaluate((el) => getComputedStyle(el).backgroundColor)).toBe(SO_RED);
      const circle = await send.evaluate((el) => {
        const box = el.getBoundingClientRect();
        return { background: getComputedStyle(el).backgroundColor, width: box.width, height: box.height };
      });
      expect(circle.background).toBe(SO_RED);
      expect(circle.width).toBeLessThanOrEqual(40);
      expect(circle.height).toBeLessThanOrEqual(40);
      expect(await send.evaluate((el) => getComputedStyle(el, "::before").content)).toBe("none");
    });

    test("answer prose is 16px / 28px, h2 and h3 weigh 600 and track -0.025em", async ({ page }) => {
      await openConsentedChat(page, theme);
      await page.getByRole("button", { name: "Start chatting" }).click();
      await page.getByRole("textbox", { name: "Your message" }).fill("show me headings");
      await page.getByRole("button", { name: "Send" }).click();

      const main = page.locator("main#view-root");
      const h2 = main.getByRole("heading", { name: "Second-level heading" });
      const h3 = main.getByRole("heading", { name: "Third-level heading" });
      await expect(h2).toBeVisible();
      const paragraph = main.getByText("A paragraph of answer prose");
      const style = (el: Locator) =>
        el.evaluate((node) => {
          const cs = getComputedStyle(node);
          return {
            fontSize: cs.fontSize,
            lineHeight: cs.lineHeight,
            fontWeight: cs.fontWeight,
            letterSpacing: cs.letterSpacing,
          };
        });

      const p = await style(paragraph);
      expect(p.fontSize).toBe("16px");
      expect(p.lineHeight).toBe("28px");
      const li = await style(main.getByText("a list item"));
      expect(li.fontSize).toBe("16px");
      expect(li.lineHeight).toBe("28px");

      const h2s = await style(h2);
      expect(h2s.fontWeight).toBe("600");
      expect(h2s.fontSize).toBe("30px"); // 1.875rem
      expect(h2s.letterSpacing).toBe("-0.75px"); // -0.025em × 30px
      const h3s = await style(h3);
      expect(h3s.fontWeight).toBe("600");
      expect(h3s.fontSize).toBe("24px"); // 1.5rem
      expect(h3s.letterSpacing).toBe("-0.6px"); // -0.025em × 24px

      // The app-wide base is unchanged: the document is still 15px.
      expect(await page.evaluate(() => getComputedStyle(document.body).fontSize)).toBe("15px");
    });

    test("the Activity fold keeps its compact size beside the 16px answer", async ({ page }) => {
      await openConsentedChat(page, theme, turnBody(true));
      await page.getByRole("button", { name: "Start chatting" }).click();
      await page.getByRole("textbox", { name: "Your message" }).fill("who calls it?");
      await page.getByRole("button", { name: "Send" }).click();

      const main = page.locator("main#view-root");
      await expect(main.getByRole("heading", { name: "Second-level heading" })).toBeVisible();
      // The fold auto-collapses onto the answer; open it to read the step result.
      await main.getByText("Activity", { exact: true }).click();
      const stepProse = main.getByText("Step result prose.");
      await expect(stepProse).toBeVisible();
      const fold = await stepProse.evaluate((node) => getComputedStyle(node).fontSize);
      expect(fold, "a step result stays at --text-sm (13px), not the 16px answer scale").toBe("13px");
      const stepHeading = await main
        .getByRole("heading", { name: "Step heading" })
        .evaluate((node) => getComputedStyle(node).fontSize);
      expect(stepHeading, "the answer's 30px h2 does not reach the fold").not.toBe("30px");
      // …while the answer beside it is still the 16px chat scale.
      expect(await main.getByText("A paragraph of answer prose").evaluate((n) => getComputedStyle(n).fontSize)).toBe("16px");
    });

    test("h5 and h6 never fall below the 16px prose", async ({ page }) => {
      const small = `event: final_answer\ndata: ${JSON.stringify({ answer: "##### Fifth\n\n###### Sixth\n\nBody." })}\n\n`;
      await openConsentedChat(page, theme, small);
      await page.getByRole("button", { name: "Start chatting" }).click();
      await page.getByRole("textbox", { name: "Your message" }).fill("small headings");
      await page.getByRole("button", { name: "Send" }).click();
      const main = page.locator("main#view-root");
      for (const name of ["Fifth", "Sixth"]) {
        const h = main.getByRole("heading", { name });
        await expect(h).toBeVisible();
        expect(await h.evaluate((n) => getComputedStyle(n).fontSize)).toBe("16px");
      }
    });

    test("no chat element larger than 40x40px is filled with the signal red", async ({ page }) => {
      await openConsentedChat(page, theme);
      await page.getByRole("button", { name: "Start chatting" }).click();
      await page.getByRole("textbox", { name: "Your message" }).fill("hello");
      await page.getByRole("button", { name: "Send" }).click();
      await expect(page.locator("main#view-root").getByRole("heading", { name: "Second-level heading" })).toBeVisible();

      const offenders = await page.evaluate((red) => {
        const out: string[] = [];
        for (const el of document.querySelectorAll("main#view-root *")) {
          if (getComputedStyle(el).backgroundColor !== red) continue;
          const box = el.getBoundingClientRect();
          if (box.width > 40 || box.height > 40) out.push(`${el.tagName}.${el.className} ${box.width}x${box.height}`);
        }
        return out;
      }, SO_RED);
      expect(offenders, "red-filled elements larger than 40x40px").toEqual([]);
    });
  });
}
