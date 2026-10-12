// The full-height chat, read in a real browser (S-619, CR-204 L2/L3/L6/C8,
// FR-UI-45): at three desktop widths and in both themes, on a 200-message thread,
// the page does not scroll, the composer is fully in view, the transcript is the
// ONLY scroller and sits at the bottom, answer prose is capped by `--measure-prose`
// and starts on the transcript's content edge, and a new answer still lands with
// the transcript following it. Shared by the member chat (`/chat`, project fixture)
// and the Workspace Chat (`/workspace-chat`, workspace fixture), which render the
// same surface.
//
// The thread, the configuration and the turn are mocked — nothing is sent
// anywhere; the bundle, its stylesheet and the CSP are the served ones.
import { expect, type Page } from "@playwright/test";

import { configuredChat } from "./views.ts";

/** The chat a spec drives: its route, the read-model it is configured through and
 *  its client-state scope and API trees. */
export interface ChatSurface {
  path: "/chat" | "/workspace-chat";
  configPath: string;
  threadsRoute: string;
  turnRoute: string;
  storageScope: string;
}

export const MEMBER_CHAT: ChatSurface = {
  path: "/chat",
  configPath: "/api/v1/config",
  threadsRoute: "/api/v1/chat/threads",
  turnRoute: "/chat",
  storageScope: "single",
};

export const WORKSPACE_CHAT: ChatSurface = {
  path: "/workspace-chat",
  configPath: "/api/v1/workspace/config",
  threadsRoute: "/api/v1/workspace/chat/threads",
  turnRoute: "/workspace/chat",
  storageScope: "workspace",
};

/** The three widths the AC names, each at a common desktop height. */
export const VIEWPORTS = [
  { width: 1024, height: 768 },
  { width: 1600, height: 900 },
  { width: 2560, height: 1440 },
] as const;

/** A paragraph long enough to fill any measure several times over. */
export const LONG_PROSE =
  "The reading measure holds a long paragraph of answer prose to a comfortable line length, " +
  "so the eye never travels the whole width of a 2560px card to find the start of the next line. ".repeat(4);

const THREAD_ID = 1;
const OLDER_THREAD_ID = 2;

// `--so-red`, as the browser serializes it.
const SO_RED = "rgb(218, 41, 28)";

/** A persisted 200-message thread: 100 question/answer pairs, the last answer a long
 *  paragraph whose line box the measure check reads. */
function longThread() {
  return Array.from({ length: 200 }, (_, i) => ({
    id: i + 1,
    role: i % 2 === 0 ? "user" : "assistant",
    content:
      i === 199 ? LONG_PROSE : i % 2 === 0 ? `Question ${i / 2 + 1}: who calls this?` : `Answer ${(i + 1) / 2}. ${LONG_PROSE.slice(0, 140)}`,
    created_at: i,
    tool_traces: [],
  }));
}

/** The turn's SSE: the answer streamed in deltas, then finalised. */
function streamedTurn(answer: string): string {
  const words = answer.split(" ");
  const deltas = words.map((w, i) => `event: answer_delta\ndata: ${JSON.stringify({ delta: (i ? " " : "") + w })}\n\n`);
  return `${deltas.join("")}event: final_answer\ndata: ${JSON.stringify({ answer })}\n\n`;
}

/** Opens `surface` consented, in `theme`, at `viewport`, restored onto the 200-message
 *  thread, and waits until its last answer is rendered. */
export async function openLongThread(
  page: Page,
  surface: ChatSurface,
  theme: "dark" | "light",
  viewport: { width: number; height: number },
): Promise<void> {
  await page.setViewportSize(viewport);
  await configuredChat(page, surface.configPath);
  await page.route(`**${surface.threadsRoute}`, (route) =>
    route.request().method() === "GET"
      ? route.fulfill({
          json: [
            { id: THREAD_ID, title: "A long conversation", updated_at: 200 },
            { id: OLDER_THREAD_ID, title: "An older conversation", updated_at: 100 },
          ],
        })
      : route.fallback(),
  );
  await page.route(`**${surface.threadsRoute}/${THREAD_ID}`, (route) =>
    route.request().method() === "GET" ? route.fulfill({ json: longThread() }) : route.fallback(),
  );
  await page.route(`**${surface.turnRoute}`, (route) =>
    route.request().method() === "POST"
      ? route.fulfill({ status: 200, contentType: "text/event-stream", body: streamedTurn(LONG_PROSE) })
      : route.fallback(),
  );
  // The persisted selection and the theme, exactly as a returning reader has them.
  await page.addInitScript(
    ([scope, id, t]) => {
      window.localStorage.setItem(`logos.chat.activeThread:${scope}`, String(id));
      window.localStorage.setItem("logos-theme", t);
      document.documentElement.setAttribute("data-theme", t);
    },
    [surface.storageScope, THREAD_ID, theme] as const,
  );
  await page.goto(surface.path);
  await page.evaluate((t) => document.documentElement.setAttribute("data-theme", t), theme);
  // Consent is this scope's own, so a fresh browser always asks: accept it, and wait
  // for the one-line chip that replaces the disclosure (CR-204 C8).
  await page.getByRole("button", { name: "Start chatting" }).click();
  await expect(page.getByRole("button", { name: /budget \d+\/\d+\/\d+/ })).toBeVisible();
  await expect(page.locator("main#view-root").getByText("Question 100: who calls this?")).toBeVisible();
}

/** What the layout check reads off the page. */
export interface LayoutReading {
  innerHeight: number;
  pageScrollHeight: number;
  composer: { top: number; bottom: number; left: number; right: number };
  scrollers: string[];
  transcriptFromBottom: number;
  prose: { left: number; width: number; contentLeft: number; measure: number };
}

/** Reads the page's scroll state, the composer's box, every element that scrolls,
 *  and the last answer's prose box against the transcript's content edge. */
export async function readLayout(page: Page): Promise<LayoutReading> {
  return page.evaluate(() => {
    const doc = document.scrollingElement ?? document.documentElement;
    const composer = document.querySelector("main#view-root form")!.getBoundingClientRect();
    const scrollers = [...document.querySelectorAll<HTMLElement>("body *")]
      .filter((el) => {
        const cs = getComputedStyle(el);
        return /(auto|scroll)/.test(cs.overflowY) && el.scrollHeight > el.clientHeight + 1;
      })
      .map((el) => `${el.tagName.toLowerCase()}.${[...el.classList].join(".")}`);
    const input = document.querySelector("main#view-root textarea")!;
    const form = input.closest("form")!;
    const transcript = form.previousElementSibling as HTMLElement;
    const paragraphs = [...transcript.querySelectorAll("p")];
    const last = paragraphs.filter((p) => (p.textContent ?? "").startsWith("The reading measure")).at(-1)!;
    // The answer's prose wrapper (`.final`), whose computed `max-width` is the measure.
    const final = last.closest('[class*="_final_"]')!;
    const cs = getComputedStyle(transcript);
    return {
      innerHeight: window.innerHeight,
      pageScrollHeight: doc.scrollHeight,
      composer: { top: composer.top, bottom: composer.bottom, left: composer.left, right: composer.right },
      scrollers,
      transcriptFromBottom: transcript.scrollHeight - transcript.scrollTop - transcript.clientHeight,
      prose: {
        left: last.getBoundingClientRect().left,
        width: last.getBoundingClientRect().width,
        contentLeft: transcript.getBoundingClientRect().left + parseFloat(cs.paddingLeft),
        measure: parseFloat(getComputedStyle(final).maxWidth),
      },
    };
  });
}

/** The S-619 layout criteria over one reading. */
export function expectFullHeightLayout(r: LayoutReading, label: string): void {
  expect(r.pageScrollHeight, `${label}: the page does not scroll`).toBeLessThanOrEqual(r.innerHeight);
  expect(r.composer.top, `${label}: the composer's top is in view`).toBeGreaterThanOrEqual(0);
  expect(r.composer.bottom, `${label}: the composer's bottom is in view`).toBeLessThanOrEqual(r.innerHeight);
  expect(r.scrollers, `${label}: the transcript is the only scroller`).toHaveLength(1);
  expect(r.scrollers[0], `${label}: the scroller is the transcript`).toMatch(/_log_/);
  expect(r.transcriptFromBottom, `${label}: the transcript sits at the bottom`).toBeLessThanOrEqual(2);
  expect(r.prose.measure, `${label}: the measure resolves to a length`).toBeGreaterThan(0);
  expect(r.prose.width, `${label}: prose is capped by --measure-prose`).toBeLessThanOrEqual(r.prose.measure + 0.5);
  expect(Math.abs(r.prose.left - r.prose.contentLeft), `${label}: prose starts on the transcript's content edge`).toBeLessThanOrEqual(
    0.5,
  );
}

/** The pill composer, the cardless rail, the consent chip and the remembered rail
 *  collapse, read as computed style on the opened long thread (S-619, FR-UI-45). */
export async function expectPillRailAndChip(page: Page, surface: ChatSurface): Promise<void> {
  const main = page.locator("main#view-root");
  const input = page.getByRole("textbox", { name: "Your message" });
  const pill = main.locator("form");
  const send = page.getByRole("button", { name: "Send" });

  // ── The pill: its radius is the pill token, and the focus ring is the pill's.
  expect(await pill.evaluate((el) => getComputedStyle(el).borderTopLeftRadius)).toBe("999px");
  await input.focus();
  const ring = await pill.evaluate((el) => ({ style: getComputedStyle(el).outlineStyle, color: getComputedStyle(el).outlineColor }));
  expect(ring, "the focus ring is drawn on the pill").toEqual({ style: "solid", color: SO_RED });
  expect(await input.evaluate((el) => getComputedStyle(el).outlineStyle), "the input draws no ring of its own").toBe("none");
  await expect(page.getByRole("button", { name: "Composer options" })).toBeVisible();

  // ── Send: neutral while empty, the red circle once there is something to send.
  expect(await send.evaluate((el) => getComputedStyle(el).backgroundColor)).not.toBe(SO_RED);
  await input.fill("x");
  // Polled: the fill eases in over `--motion-fast`.
  await expect.poll(() => send.evaluate((el) => getComputedStyle(el).backgroundColor)).toBe(SO_RED);
  // Keyboard focus keeps the circle: the global ring's radius must not square it.
  await input.focus();
  await page.keyboard.press("Tab");
  await expect(send).toBeFocused();
  expect(await send.evaluate((el) => getComputedStyle(el).borderTopLeftRadius), "a focused Send stays round").toBe("999px");

  // ── The input grows to a cap, then scrolls; the page still holds still.
  await input.fill(Array.from({ length: 30 }, (_, i) => `line ${i + 1}`).join("\n"));
  const grown = await input.evaluate((el) => ({ height: el.clientHeight, scroll: el.scrollHeight }));
  expect(grown.scroll, "past the cap the input scrolls").toBeGreaterThan(grown.height);
  expect(grown.height, "the cap holds the input to a few lines").toBeLessThanOrEqual(240);
  const pageAfter = await page.evaluate(() => (document.scrollingElement ?? document.documentElement).scrollHeight - window.innerHeight);
  expect(pageAfter, "a grown input never scrolls the page").toBeLessThanOrEqual(0);
  await input.fill("");

  // ── The rail: no card; the open row is a filled pill with a red leading marker
  // and no outline; an idle row's trash is hidden until the row is hovered.
  const rail = page.getByRole("navigation", { name: "Conversations" });
  const railStyle = await rail.evaluate((el) => {
    const cs = getComputedStyle(el);
    return { background: cs.backgroundColor, border: cs.borderTopWidth, shadow: cs.boxShadow, padding: cs.paddingLeft };
  });
  expect(railStyle).toEqual({ background: "rgba(0, 0, 0, 0)", border: "0px", shadow: "none", padding: "0px" });
  const activeTitle = rail.getByRole("button", { name: "A long conversation", exact: true });
  const activeLine = activeTitle.locator("..");
  const active = await activeLine.evaluate((el) => {
    const cs = getComputedStyle(el);
    const marker = getComputedStyle(el, "::before");
    return {
      fill: cs.backgroundColor,
      outline: cs.outlineStyle,
      lineBorder: cs.borderTopWidth,
      marker: { background: marker.backgroundColor, width: marker.width },
    };
  });
  expect(active.fill, "the open row is a filled pill").not.toBe("rgba(0, 0, 0, 0)");
  expect(active.marker, "the open row carries the red leading marker").toEqual({ background: SO_RED, width: "3px" });
  expect(active.outline, "and no outline").toBe("none");
  expect(active.lineBorder).toBe("0px");
  expect(await activeTitle.evaluate((el) => getComputedStyle(el).borderTopWidth), "no outline box on the title either").toBe("0px");

  await page.mouse.move(1, 1);
  const idleTrash = page.getByRole("button", { name: "Delete conversation “An older conversation”" });
  await expect.poll(() => idleTrash.evaluate((el) => getComputedStyle(el).opacity), { message: "an idle row's trash is hidden" }).toBe("0");
  await rail.getByRole("button", { name: "An older conversation", exact: true }).hover();
  await expect.poll(() => idleTrash.evaluate((el) => getComputedStyle(el).opacity), { message: "hovering the row reveals it" }).toBe("1");
  await idleTrash.click();
  await expect(page.getByRole("group", { name: "Confirm deleting “An older conversation”" })).toBeVisible();
  await page.getByRole("button", { name: "Cancel" }).click();

  // ── The chip: one line high, expanding to the band with the budget bounds.
  const chip = page.getByRole("button", { name: /budget \d+\/\d+\/\d+/ });
  const chipBox = await chip.boundingBox();
  expect(chipBox!.height, "the post-consent lead is one line high").toBeLessThanOrEqual(40);
  await chip.click();
  await expect(chip).toHaveAttribute("aria-expanded", "true");
  await expect(main.getByText(/Budget tree: \d+ tool calls/)).toBeVisible();

  // ── The collapse toggle: it hides the rail, and the choice survives a reload.
  await page.getByRole("button", { name: "Hide conversations" }).click();
  await expect(rail).toBeHidden();
  await page.reload();
  await expect(page.getByRole("button", { name: "Show conversations" })).toHaveAttribute("aria-expanded", "false");
  expect(await page.evaluate((scope) => window.localStorage.getItem(`logos.chat.railOpen:${scope}`), surface.storageScope)).toBe("0");
}
