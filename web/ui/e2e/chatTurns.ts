// The chat turns' last layer, read in a real browser (S-620, CR-204 L4/C1/C2/C10,
// FR-UI-45, FR-UI-46): the user bubble's 24px radius and 10px × 20px inset, the 24px
// icon action bar and its hover/focus tint, Copy's check, the floating ↓ control, and
// the four keyboard shortcuts — in both themes, for the member chat and the Workspace
// Chat alike (one surface, `ChatConfigured`). Shared by the two specs that name them.
//
// The thread, the configuration and the turns are mocked — nothing is sent anywhere;
// the bundle, its stylesheet and the CSP are the served ones. It reuses S-619's 200-
// message thread (`openLongThread`), so there is a transcript long enough to scroll.
import { expect, type Locator, type Page } from "@playwright/test";

import { openLongThread, type ChatSurface } from "./chatLayout.ts";

const DESKTOP = { width: 1600, height: 900 } as const;

/** The transcript's distance from its own bottom — 0 when it is following the end. */
async function fromBottom(page: Page): Promise<number> {
  return page.evaluate(() => {
    const form = document.querySelector("main#view-root textarea")!.closest("form")!;
    const t = form.previousElementSibling as HTMLElement;
    return t.scrollHeight - t.scrollTop - t.clientHeight;
  });
}

/** Scrolls the transcript to `top` the way a reader does (a real scroll event). */
async function scrollTranscript(page: Page, top: number): Promise<void> {
  await page.evaluate((y) => {
    const form = document.querySelector("main#view-root textarea")!.closest("form")!;
    (form.previousElementSibling as HTMLElement).scrollTop = y;
  }, top);
}

/** The colour `--tint-hover` computes to in the page's current theme. */
async function tintHover(page: Page): Promise<string> {
  return page.evaluate(() => {
    const probe = document.createElement("div");
    probe.style.backgroundColor = "var(--tint-hover)";
    document.body.appendChild(probe);
    const colour = getComputedStyle(probe).backgroundColor;
    probe.remove();
    return colour;
  });
}

const bg = (l: Locator) => l.evaluate((el) => getComputedStyle(el).backgroundColor);

export async function expectBubbleAndActionBar(page: Page, surface: ChatSurface, theme: "dark" | "light"): Promise<void> {
  await openLongThread(page, surface, theme, DESKTOP);

  // The user bubble: 24px radius, 10px × 20px inset.
  const bubble = page.locator("main#view-root").getByText("Question 100: who calls this?");
  const style = await bubble.evaluate((el) => {
    const b = el.closest('[class*="_userBubble_"]')!;
    const cs = getComputedStyle(b);
    return { radius: cs.borderTopLeftRadius, pad: [cs.paddingTop, cs.paddingRight, cs.paddingBottom, cs.paddingLeft] };
  });
  expect(style.radius).toBe("24px");
  expect(style.pad).toEqual(["10px", "20px", "10px", "20px"]);

  // The assistant turn stays flat: no fill, no shadow (the card-chrome guard's twin).
  const turn = await page.locator('[class*="_assistant_"]').last().evaluate((el) => {
    const cs = getComputedStyle(el);
    return { bg: cs.backgroundColor, shadow: cs.boxShadow };
  });
  expect(turn.bg).toBe("rgba(0, 0, 0, 0)");
  expect(turn.shadow).toBe("none");

  // Copy and Regenerate: 24px icon buttons, ghost at rest, --tint-hover on hover AND focus.
  const tint = await tintHover(page);
  for (const name of ["Copy answer", "Regenerate answer"]) {
    const button = page.getByRole("button", { name }).last();
    await expect(button).toHaveAttribute("title", name);
    const box = await button.boundingBox();
    expect(Math.round(box!.width), `${name} width`).toBe(24);
    expect(Math.round(box!.height), `${name} height`).toBe(24);
    expect(await bg(button), `${name} at rest`).toBe("rgba(0, 0, 0, 0)");

    await button.hover();
    await expect.poll(() => bg(button), { message: `${name} hovered` }).toBe(tint);
    await page.mouse.move(0, 0);
    await expect.poll(() => bg(button), { message: `${name} after hover` }).toBe("rgba(0, 0, 0, 0)");

    // A key press first, so a scripted focus counts as keyboard focus (:focus-visible).
    await page.keyboard.press("Shift");
    await button.focus();
    await expect.poll(() => bg(button), { message: `${name} focused` }).toBe(tint);
    await button.blur();
  }

  // Copy shows a check for 1.5 s, then the copy glyph again.
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  const copy = page.getByRole("button", { name: "Copy answer" }).last();
  const idle = await copy.innerHTML();
  await copy.click();
  await expect(copy).toHaveAttribute("data-copied", "true");
  expect(await copy.innerHTML()).not.toBe(idle);
  expect(await page.evaluate(() => navigator.clipboard.readText())).toContain("The reading measure");
  await expect(copy).not.toHaveAttribute("data-copied", "true", { timeout: 4_000 });
  expect(await copy.innerHTML()).toBe(idle);
}

export async function expectScrollToBottom(page: Page, surface: ChatSurface, theme: "dark" | "light"): Promise<void> {
  await openLongThread(page, surface, theme, DESKTOP);
  const down = page.getByRole("button", { name: "Scroll to bottom" });

  // At the bottom: not in the accessibility tree at all.
  await expect.poll(() => fromBottom(page)).toBeLessThanOrEqual(2);
  await expect(down).toHaveCount(0);

  // Scrolled away: the control appears, round and clear of the composer.
  await scrollTranscript(page, 0);
  await expect(down).toBeVisible();
  const [box, composer] = await Promise.all([
    down.boundingBox(),
    page.locator("main#view-root textarea").evaluate((el) => el.closest("form")!.getBoundingClientRect().top),
  ]);
  expect(Math.round(box!.width)).toBe(32);
  expect(Math.round(box!.height)).toBe(32);
  expect(box!.y + box!.height, "it floats above the composer").toBeLessThanOrEqual(composer);
  expect(await down.evaluate((el) => getComputedStyle(el).borderRadius)).not.toBe("0px");

  // Activating it returns to the bottom, and the control leaves the tree.
  await down.click();
  await expect.poll(() => fromBottom(page)).toBeLessThanOrEqual(2);
  await expect(down).toHaveCount(0);

  // …and the stream is followed again: a new answer lands with the end in view.
  await page.getByRole("textbox", { name: "Your message" }).fill("one more?");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page.locator("main#view-root").getByText("one more?")).toBeVisible();
  await expect(page.getByRole("button", { name: "Send" })).toBeVisible();
  await expect.poll(() => fromBottom(page)).toBeLessThanOrEqual(2);
  await expect(down).toHaveCount(0);
}

export async function expectShortcuts(page: Page, surface: ChatSurface, theme: "dark" | "light"): Promise<void> {
  // A turn that stays open until released, to give Esc something to stop.
  let release: () => void = () => {};
  const gate = new Promise<void>((r) => (release = r));
  let posts = 0;
  await openLongThread(page, surface, theme, DESKTOP);
  // Registered after `openLongThread`'s own turn route, so it takes precedence for POST.
  await page.route(`**${surface.turnRoute}`, async (route) => {
    if (route.request().method() !== "POST") return route.fallback();
    posts += 1;
    await gate;
    await route
      .fulfill({
        status: 200,
        contentType: "text/event-stream",
        body: `event: final_answer\ndata: ${JSON.stringify({ answer: "Held answer." })}\n\n`,
      })
      .catch(() => {});
  });

  const input = page.getByRole("textbox", { name: "Your message" });

  // The hint names Enter and Shift+Enter.
  const hint = page.locator("main#view-root").getByText(/to send/);
  await expect(hint).toContainText("Enter");
  await expect(hint).toContainText("Shift");
  await expect(hint).toContainText("new line");

  // Shift+Enter inserts a newline and sends nothing; Enter sends.
  await input.click();
  await input.pressSequentially("first line");
  await page.keyboard.press("Shift+Enter");
  await input.pressSequentially("second line");
  await expect(input).toHaveValue("first line\nsecond line");
  expect(posts).toBe(0);
  await page.keyboard.press("Enter");
  await expect(page.getByRole("button", { name: "Stop" })).toBeVisible();
  expect(posts).toBe(1);

  // Esc stops the running turn — the pill goes back to Send. Focus leaves the textarea
  // first: assistant-ui's own input also cancels on Esc, which would pass this step
  // even with the shortcut removed. From the transcript only the shortcut can hear it.
  await page.getByRole("heading", { name: "Conversation" }).click();
  await expect(input).not.toBeFocused();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("button", { name: "Send" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Stop" })).toHaveCount(0);
  release();

  // ⌘K / Ctrl+K from a text field of ANOTHER view (one outside the chat) does nothing…
  await page.evaluate(() => {
    const other = document.createElement("input");
    other.id = "foreign-field";
    other.setAttribute("aria-label", "another view's field");
    document.body.appendChild(other);
  });
  // …observed as "was the key claimed?" (a page listener after the chat's own), which
  // settles at once, instead of waiting for a reset that must not happen.
  await page.evaluate(() => {
    (window as unknown as { __claimed: boolean[] }).__claimed = [];
    document.addEventListener("keydown", (e) => (window as unknown as { __claimed: boolean[] }).__claimed.push(e.defaultPrevented));
  });
  await page.locator("#foreign-field").focus();
  await page.keyboard.press("ControlOrMeta+k");
  expect(await page.evaluate(() => (window as unknown as { __claimed: boolean[] }).__claimed.at(-1))).toBe(false);
  await expect(page.locator("main#view-root").getByText("Question 100: who calls this?")).toBeVisible();

  // …and from the chat starts a new one: the restored conversation is gone.
  await input.click();
  await page.keyboard.press("ControlOrMeta+k");
  expect(await page.evaluate(() => (window as unknown as { __claimed: boolean[] }).__claimed.at(-1))).toBe(true);
  await expect(page.locator("main#view-root").getByText("Question 100: who calls this?")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Send" })).toBeVisible();
}
