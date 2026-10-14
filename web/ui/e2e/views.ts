// Shared steps for the layout specs (S-617; `widgetTitles` is every spec's one title reader): open a view, wait for
// the widget that says it has rendered, and hand back its one visible stack.
import { expect, type APIRequestContext, type Locator, type Page } from "@playwright/test";

import type { ExtensionsInfo } from "../src/api/extensionsClient.ts";

export interface ViewCase {
  /** A widget title whose heading proves the view has rendered its widgets. */
  ready: string;
  /** The view stacks a single child (no gap to measure). */
  single?: boolean;
  /** Routes to set up before the view is opened. */
  setup?: (page: Page) => Promise<void>;
}

/**
 * The routes the served build's ACTIVE extensions add to the sidebar of `scope` —
 * read from the server's own `GET /api/v1/extensions`, the answer the SPA mounts from
 * (S-642, FR-UI-51). The specs add them to the built-in navigation (`navItemsFor`,
 * `WORKSPACE_NAV_ITEMS`) to get the COMPOSED navigation their cases must cover; the
 * public build reports none, so there it is the built-in navigation unchanged.
 *
 * It lists the manifest as the server sent it. The SPA mounts an entry only when the
 * bundle carries its view and a built-in does not already own its id or route
 * (`registry.ts`, `nav.ts`), so a composed build whose manifest carries such an entry
 * must filter it here too.
 */
export async function extensionNavPaths(
  request: APIRequestContext,
  scope: "member" | "app",
): Promise<string[]> {
  const response = await request.get("/api/v1/extensions");
  expect(response.ok(), "GET /api/v1/extensions").toBe(true);
  const info = (await response.json()) as ExtensionsInfo;
  return info.extensions.flatMap((e) => e.ui?.nav ?? []).filter((n) => n.scope === scope).map((n) => n.path);
}

/** Does the served build compose no extension at all (`GET /api/v1/extensions`
 *  lists none) — the public build? */
export async function composesNoExtension(request: APIRequestContext): Promise<boolean> {
  const response = await request.get("/api/v1/extensions");
  expect(response.ok(), "GET /api/v1/extensions").toBe(true);
  return ((await response.json()) as ExtensionsInfo).extensions.length === 0;
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
