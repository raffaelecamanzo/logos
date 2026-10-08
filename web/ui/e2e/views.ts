// Shared steps for the every-view layout specs (S-617): open a view, wait for
// the widget that says it has rendered, and hand back its one visible stack.
import { expect, type Locator, type Page } from "@playwright/test";

export interface ViewCase {
  /** A widget title whose heading proves the view has rendered its widgets. */
  ready: string;
  /** The view stacks a single child (no gap to measure). */
  single?: boolean;
  /** Routes to set up before the view is opened. */
  setup?: (page: Page) => Promise<void>;
}

/** Every widget's title in the stack, in order (a gloss's tooltip left out). */
export async function widgetTitles(stack: Locator): Promise<string[]> {
  return stack.locator("[data-widget] > [data-widget-part='title'] h3").evaluateAll((nodes) =>
    nodes.map((h) => {
      const clone = h.cloneNode(true) as Element;
      clone.querySelectorAll('[role="tooltip"]').forEach((tip) => tip.remove());
      return (clone.textContent ?? "").replace(/\s+/g, " ").trim();
    }),
  );
}

/** Opens `path`, waits for `view.ready`, and returns the page's one visible stack. */
export async function layoutCase(page: Page, path: string, view: ViewCase) {
  if (view.setup) await view.setup(page);
  await page.goto(path);
  const main = page.locator("main#view-root");
  await expect(main.getByRole("heading", { name: view.ready, exact: true }).first()).toBeVisible();
  // A tabbed view renders every tab's stack and hides the inactive ones.
  const stack = main.locator("[data-widget-stack]:visible");
  await expect(stack, "one visible WidgetStack per view").toHaveCount(1);
  const titles = await widgetTitles(stack);
  expect(titles, "the view's widgets").toContain(view.ready);
  return { stack, titles };
}

/**
 * Serves the chat configuration read as CONFIGURED: the real answer, with its
 * effective chat set to a declared model and a present key. The chat then shows
 * its consent banner and the conversation panel. Nothing is sent anywhere: no
 * question is asked, and the composer stays disabled until consent.
 */
export async function configuredChat(page: Page, path: string): Promise<void> {
  await page.route(`**${path}`, async (route) => {
    if (route.request().method() !== "GET") return route.fallback();
    const response = await route.fetch();
    const body = (await response.json()) as Record<string, unknown>;
    body.effective_chat = {
      policy: {
        provider: "openai",
        model: "e2e-model",
        base_url: "https://example.invalid/v1",
        max_tool_calls: 24,
        max_subagent_tool_calls: 8,
        max_replans: 3,
      },
      policy_origin: path.includes("workspace") ? "workspace" : "member",
      credential: { present: true, last4: "e2e0" },
      credential_origin: path.includes("workspace") ? "workspace" : "member",
      member_key_withheld: false,
    };
    await route.fulfill({ response, json: body });
  });
}
