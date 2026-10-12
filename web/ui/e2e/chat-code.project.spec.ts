// Chat code blocks and verified code references, read in a real browser (S-622,
// CR-204 C4, FR-UI-47): syntax colours computed from the served stylesheet in BOTH
// themes, no `style` attribute and no CSP violation, and the code references the
// fixture's REAL graph resolves — the one resolution GET is not mocked, only the
// turn is (no outbound call, no provider), as in `chat-prose.project.spec.ts`.
//
// The fixture (`e2e/fixtures/single`) indexes `src/main.rs` with the functions
// `greeting` (one node) and `lookup` (a function AND the module `lookup.rs`, so
// the bare name is ambiguous).
import { expect, test, type Page } from "@playwright/test";

import { configuredChat } from "./views.ts";

const ANSWER = [
  "The entry point is `src/main.rs:17`; it prints `greeting`.",
  "A name like `lookup` is ambiguous here, and `src/missing.rs` does not exist.",
  "",
  "```rust",
  "fn main() {",
  '    let name = "logos"; // the fixture',
  "    println!(\"{}\", 42);",
  "}",
  "```",
  "",
  "```brainfuck",
  "+[->+<]",
  "```",
].join("\n");

const TURN = `event: final_answer\ndata: ${JSON.stringify({ answer: ANSWER })}\n\n`;

/** The hex syntax colours of `tokens.css`, per theme, as the browser serializes them. */
const SYNTAX = {
  dark: { keyword: "rgb(199, 146, 234)", string: "rgb(158, 206, 106)", number: "rgb(242, 166, 90)", comment: "rgb(139, 149, 165)" },
  light: { keyword: "rgb(124, 58, 237)", string: "rgb(21, 128, 61)", number: "rgb(180, 83, 9)", comment: "rgb(106, 111, 120)" },
} as const;

/** Open the chat, consent, ask, and wait for the finalized answer. Returns every
 *  code-reference request the page made and every CSP violation it reported. */
async function askForCode(page: Page, theme: "dark" | "light") {
  const codeRefRequests: string[] = [];
  const codeRefAnswers: Promise<{ refs: { text: string; outcome: string }[] }>[] = [];
  page.on("request", (req) => {
    if (new URL(req.url()).pathname === "/api/v1/code-refs") codeRefRequests.push(req.url());
  });
  page.on("response", (res) => {
    if (new URL(res.url()).pathname === "/api/v1/code-refs") codeRefAnswers.push(res.json());
  });
  await configuredChat(page, "/api/v1/config");
  await page.route("**/chat", async (route) => {
    if (route.request().method() !== "POST") return route.fallback();
    await route.fulfill({ status: 200, contentType: "text/event-stream", body: TURN });
  });
  await page.goto("/chat");
  await page.evaluate(() => {
    (window as unknown as { cspViolations: string[] }).cspViolations = [];
    document.addEventListener("securitypolicyviolation", (e) =>
      (window as unknown as { cspViolations: string[] }).cspViolations.push(e.violatedDirective),
    );
  });
  await page.evaluate((t) => document.documentElement.setAttribute("data-theme", t), theme);
  await page.getByRole("button", { name: "Start chatting" }).click();
  await page.getByRole("textbox", { name: "Your message" }).fill("where does it start?");
  await page.getByRole("button", { name: "Send" }).click();
  const main = page.locator("main#view-root");
  await expect(main.getByText("The entry point is")).toBeVisible();
  return { main, codeRefRequests, codeRefAnswers };
}

for (const theme of ["dark", "light"] as const) {
  test.describe(`${theme} theme`, () => {
    test("a rust block is coloured from the syntax tokens; an unknown language is plain", async ({ page }) => {
      const { main } = await askForCode(page, theme);
      const rust = main.locator("pre code").first();
      const colour = (selector: string) => rust.locator(selector).first().evaluate((n) => getComputedStyle(n).color);
      expect(await colour(".hljs-keyword")).toBe(SYNTAX[theme].keyword);
      expect(await colour(".hljs-string")).toBe(SYNTAX[theme].string);
      expect(await colour(".hljs-number")).toBe(SYNTAX[theme].number);
      expect(await colour(".hljs-comment")).toBe(SYNTAX[theme].comment);

      const plain = main.locator("pre code").nth(1);
      await expect(plain).toHaveText("+[->+<]");
      await expect(plain.locator("span")).toHaveCount(0);

      // Class-only: nothing in the answer carries a `style` attribute, and the
      // self-only CSP reported no violation while it rendered. Scoped to the
      // answer's markdown body: assistant-ui's own user-bubble text and composer
      // textarea set inline sizing outside it, which this story does not touch.
      const answer = main.getByText("The entry point is").locator("xpath=..");
      await expect(answer.locator("pre")).toHaveCount(2);
      expect(await answer.evaluate((el) => el.querySelectorAll("[style]").length + (el.hasAttribute("style") ? 1 : 0))).toBe(0);
      expect(await page.evaluate(() => (window as unknown as { cspViolations: string[] }).cspViolations)).toEqual([]);
    });
  });
}

test("exactly the existing file and the unique symbol link, from one resolution GET", async ({ page }) => {
  const { main, codeRefRequests, codeRefAnswers } = await askForCode(page, "dark");
  const file = main.getByRole("link", { name: "src/main.rs:17" });
  const symbol = main.getByRole("link", { name: "greeting" });
  await expect(file).toBeVisible();
  await expect(symbol).toBeVisible();
  expect(await file.getAttribute("href")).toBe("/graph?seed=src%2Fmain.rs&granularity=file");
  expect(await symbol.getAttribute("href")).toMatch(/^\/graph\?seed=.*greeting/);
  // The ambiguous name and the missing path stay plain code.
  for (const plain of ["lookup", "src/missing.rs"]) {
    await expect(main.locator("code", { hasText: plain }).first()).toBeVisible();
    expect(await main.locator("a", { hasText: plain }).count(), `${plain} is not a link`).toBe(0);
  }
  expect(codeRefRequests, "one finalized turn issues one resolution GET").toHaveLength(1);
  // What the fixture's real graph answered, span by span: the two plain spans are
  // plain for the two different reasons the criterion names.
  const answer = await codeRefAnswers[0];
  expect(answer.refs.map((r) => [r.text, r.outcome])).toEqual([
    ["src/main.rs:17", "file"],
    ["greeting", "symbol"],
    ["lookup", "ambiguous"],
    ["src/missing.rs", "unresolved"],
  ]);
});

/** Click a verified link and return the Graph view's opening `/api/v1/graph` fetch.
 *  The view consumes the deep link (it leaves the URL once read), so the fetch is
 *  where the seed is observed. */
async function openLink(page: Page, name: string): Promise<URL> {
  const { main } = await askForCode(page, "dark");
  const graphFetch = page.waitForRequest((req) => new URL(req.url()).pathname === "/api/v1/graph");
  await main.getByRole("link", { name }).click();
  const url = new URL((await graphFetch).url());
  await expect(page).toHaveURL(/\/graph$/);
  return url;
}

test("a verified symbol link opens the Graph view focused on that node", async ({ page }) => {
  const fetched = await openLink(page, "greeting");
  expect(fetched.searchParams.get("seed")).toMatch(/greeting/);
  expect(fetched.searchParams.has("granularity")).toBe(false);
  await expect(page.getByText(/Focused on greeting — Reset to return/)).toBeVisible();
});

test("a verified file link opens the Graph view at the file tier, focused on the file", async ({ page }) => {
  const fetched = await openLink(page, "src/main.rs:17");
  expect(fetched.searchParams.get("seed")).toBe("src/main.rs");
  expect(fetched.searchParams.get("granularity")).toBe("file");
  await expect(page.getByText(/Focused on src\/main\.rs — Reset to return/)).toBeVisible();
});
