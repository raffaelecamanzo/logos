// The Security page's Findings table in a real browser (S-793, CR-245,
// frontend-design §4.23, FR-UI-44): at a 1024-pixel viewport with the sidebar
// visible, neither the page nor the table scrolls sideways — with a finding whose
// rule id and path are 120 characters each (`single.security.sarif`) on the page.
// And its code scope control (S-796, FR-SC-28): production by default, the
// fixture's one finding in test code listed, tagged and kept in the URL under
// Test code and All.
import { expect, test, type Page } from "@playwright/test";

const RULE = `rust.lang.security.a-deliberately-long-rule-id-${"r".repeat(73)}`;
const PATH = "src/generated-xxxxxxxxx/vendor/bindings/protocol/serialization/handlers/endpoints/versioned/internal/request_dispatch.rs";

test.use({ viewport: { width: 1024, height: 768 } });

async function openFindings(page: Page, search = "") {
  await page.goto(`/security${search}`);
  const table = page.getByRole("table", { name: "Security findings" });
  await expect(table).toBeVisible();
  return table;
}

test("the fixture's long rule id and path are 120 characters, so the fit check is not vacuous", () => {
  expect(RULE).toHaveLength(120);
  expect(PATH).toHaveLength(120);
});

// The default view, then every code scope listed: the `TEST` tag a test-code row
// carries must not push the table past its widget (S-796).
for (const [name, search] of [["by default", ""], ["with the test-code row listed (?scope=all)", "?scope=all"]] as const) {
  test(`at 1024 px with the sidebar visible, the page and the Findings table do not scroll sideways ${name}`, async ({ page }) => {
    await expectFits(page, search);
  });
}

async function expectFits(page: Page, search: string) {
  const table = await openFindings(page, search);
  await expect(page.getByRole("navigation", { name: "Views" })).toBeVisible();
  await expect(table.locator(`[data-path="${PATH}"]`)).toBeVisible();
  await expect(table.getByText(RULE, { exact: true })).toBeVisible();
  if (search !== "") await expect(table.locator("[data-code-scope='test']")).toBeVisible();

  const fit = await table.evaluate((el) => {
    const wrap = el.parentElement!; // DataTable's scroll container
    const card = el.closest("[data-widget]")!;
    const frameRight = card.getBoundingClientRect().right - parseFloat(getComputedStyle(card).paddingRight);
    return {
      wrapScroll: wrap.scrollWidth,
      wrapClient: wrap.clientWidth,
      tableRight: el.getBoundingClientRect().right,
      frameRight,
      pageScroll: document.documentElement.scrollWidth,
      pageClient: document.documentElement.clientWidth,
    };
  });
  expect(fit.wrapScroll, "the table's scroll container does not scroll sideways").toBeLessThanOrEqual(fit.wrapClient);
  expect(fit.tableRight, "the table ends inside its widget").toBeLessThanOrEqual(fit.frameRight + 0.5);
  expect(fit.pageScroll, "the page does not scroll sideways").toBeLessThanOrEqual(fit.pageClient);
}

test("the long rule id is clipped to its column, with the full id as tooltip and text", async ({ page }) => {
  const table = await openFindings(page);
  const rule = table.getByText(RULE, { exact: true });
  await expect(rule).toHaveAttribute("title", RULE);
  const clipped = await rule.evaluate((el) => ({ scroll: el.scrollWidth, client: el.clientWidth }));
  expect(clipped.scroll, "the drawn id is shorter than the id").toBeGreaterThan(clipped.client);
  expect(clipped.client, "the id is held to a column's width").toBeLessThan(300);
});

test("the long path is abbreviated, with the full path as tooltip and accessible name", async ({ page }) => {
  const table = await openFindings(page);
  const path = table.locator(`[data-path="${PATH}"]`);
  await expect(path).toHaveAttribute("title", PATH);
  await expect(path.locator('[aria-hidden="true"]')).toHaveText("src/…/internal/request_dispatch.rs");
  await expect(table.getByRole("cell", { name: new RegExp(`^${PATH} ?:1234$`) })).toBeVisible();
});

const TEST_PATH = "tests/fixtures/payments.rs";

test("the Findings table lists production code by default and the fixture finding in test code apart", async ({ page }) => {
  const table = await openFindings(page);
  await expect(page.getByLabel("Code scope")).toHaveValue("production");
  await expect(table.locator(`[data-path="${TEST_PATH}"]`)).toHaveCount(0);
  await expect(table.locator("[data-code-scope]")).toHaveCount(0);
  // The figure row states what the counts leave out, beside them.
  const findings = page.locator("section").filter({ has: page.getByRole("heading", { name: "Findings", exact: true }) });
  await expect(findings.locator('[data-widget-part="figure"]')).toContainText("· 1 in test code");
});

test("All lists the test-code finding with a muted TEST tag whose hover gives the reason, and the URL keeps the scope", async ({ page }) => {
  const table = await openFindings(page);
  await page.getByLabel("Code scope").selectOption("all");
  await expect(page).toHaveURL(/\/security\?scope=all$/);
  const row = table.getByRole("row").filter({ has: page.locator(`[data-path="${TEST_PATH}"]`) });
  await expect(row).toHaveCount(1);
  const tag = row.locator("[data-code-scope='test']");
  await expect(tag).toHaveAttribute("title", "In test code: its path is a test, fixtures or testdata path.");
  await expect(tag).toContainText("TEST");

  // A reload opens on the scope the URL names; Test code lists that finding alone.
  await page.reload();
  await expect(page.getByLabel("Code scope")).toHaveValue("all");
  await page.getByLabel("Code scope").selectOption("test");
  await expect(page).toHaveURL(/\/security\?scope=test$/);
  await expect(table.getByRole("row")).toHaveCount(2); // header and the one finding
  await page.getByLabel("Code scope").selectOption("production");
  await expect(page).toHaveURL(/\/security$/);
});

test("a redacted finding reads the explanation and its location in the detail", async ({ page }) => {
  const table = await openFindings(page);
  await table.getByRole("button", { name: /Show generic\.secrets\.hardcoded-token/ }).click();
  const detail = page.getByRole("heading", { name: "Finding detail" }).locator("xpath=ancestor::*[@data-widget][1]");
  await expect(detail).toContainText("Redacted: the scanner's message would contain the secret itself, at src/main.rs:28");
  await expect(detail).not.toContainText("[redacted]");
});
