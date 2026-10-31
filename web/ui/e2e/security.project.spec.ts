// The Security page's Findings table in a real browser (S-793, CR-245,
// frontend-design §4.23, FR-UI-44): at a 1024-pixel viewport with the sidebar
// visible, neither the page nor the table scrolls sideways — with a finding whose
// rule id and path are 120 characters each (`single.security.sarif`) on the page.
// And its code scope control (S-796, FR-SC-28): production by default, the
// fixture's one finding in test code listed, tagged and kept in the URL under
// Test code and All. And what its scanner marks low-confidence (S-817, FR-SC-31):
// left out of the default list and stated as "N more need review", listed and
// tagged REVIEW under All. And a finding on a dev-only dependency (S-810,
// FR-SC-30): left out of the default list and stated as "N more in dev-only
// dependencies", listed and tagged DEV under All. And one advisory OSV-Scanner and
// Trivy both report (S-809, FR-SC-29): one row naming both scanners, its detail
// listing each reporter.
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

const REVIEW_RULE = "rust.lang.security.string-built-query";

test("a finding its scanner marks low-confidence is out of the default list and counted as needing review (S-817)", async ({ page }) => {
  const table = await openFindings(page);
  await expect(table.getByText(REVIEW_RULE, { exact: true })).toHaveCount(0);
  await expect(table.locator("[data-needs-review]")).toHaveCount(0);
  const findings = page.locator("section").filter({ has: page.getByRole("heading", { name: "Findings", exact: true }) });
  await expect(findings.locator('[data-widget-part="figure"]')).toContainText("· 1 more needs review");
  // The OWASP figure states the same number beside its count.
  const owasp = page.locator("section").filter({ has: page.getByRole("heading", { name: "OWASP Top 10", exact: true }) });
  await expect(owasp.locator('[data-widget-part="figure"]')).toContainText("· 1 more needs review");
});

test("All lists the low-confidence finding with a muted REVIEW tag whose hover says what it changes (S-817)", async ({ page }) => {
  const table = await openFindings(page);
  await page.getByLabel("Code scope").selectOption("all");
  const row = table.getByRole("row").filter({ has: page.getByText(REVIEW_RULE, { exact: true }) });
  await expect(row).toHaveCount(1);
  const tag = row.locator("[data-needs-review]");
  await expect(tag).toContainText("REVIEW");
  await expect(tag).toHaveAttribute("title", /low-confidence.*still counts for the \[security\] rules/);
  // Test code lists test code alone: the finding is production code, so it is not there.
  await page.getByLabel("Code scope").selectOption("test");
  await expect(table.getByText(REVIEW_RULE, { exact: true })).toHaveCount(0);
});

const DEV_RULE = "GHSA-dev-only-0001";

test("a finding on a dev-only dependency is out of the default list and counted as dev-only (S-810)", async ({ page }) => {
  const table = await openFindings(page);
  await expect(table.getByText(DEV_RULE, { exact: true })).toHaveCount(0);
  await expect(table.locator("[data-code-scope='dev']")).toHaveCount(0);
  const findings = page.locator("section").filter({ has: page.getByRole("heading", { name: "Findings", exact: true }) });
  await expect(findings.locator('[data-widget-part="figure"]')).toContainText("· 1 more in dev-only dependencies");
  const owasp = page.locator("section").filter({ has: page.getByRole("heading", { name: "OWASP Top 10", exact: true }) });
  await expect(owasp.locator('[data-widget-part="figure"]')).toContainText("· 1 more in dev-only dependencies");
});

test("All lists the dev-only finding with a muted DEV tag whose hover says what it changes, and it fits (S-810)", async ({ page }) => {
  const table = await openFindings(page);
  await page.getByLabel("Code scope").selectOption("all");
  const row = table.getByRole("row").filter({ has: page.getByText(DEV_RULE, { exact: true }) });
  await expect(row).toHaveCount(1);
  const tag = row.locator("[data-code-scope='dev']");
  await expect(tag).toContainText("DEV");
  await expect(tag).toHaveAttribute("title", /Dev-only dependency.*still counts for the \[security\] rules/);
  // Neither the page nor the table scrolls sideways with the tag on a row.
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(overflow).toBeLessThanOrEqual(0);
  // Test code lists test code alone: a dev-only dependency is neither.
  await page.getByLabel("Code scope").selectOption("test");
  await expect(table.getByText(DEV_RULE, { exact: true })).toHaveCount(0);
});

test("a redacted finding reads the explanation and its location in the detail", async ({ page }) => {
  const table = await openFindings(page);
  await table.getByRole("button", { name: /Show generic\.secrets\.hardcoded-token/ }).click();
  const detail = page.getByRole("heading", { name: "Finding detail" }).locator("xpath=ancestor::*[@data-widget][1]");
  await expect(detail).toContainText("Redacted: the scanner's message would contain the secret itself, at src/main.rs:28");
  await expect(detail).not.toContainText("[redacted]");
});

const MERGED_RULE = "CVE-2026-0001";

test("one advisory OSV-Scanner and Trivy both report is one row naming both, its detail listing each (S-809)", async ({ page }) => {
  const table = await openFindings(page);
  const rows = table.getByRole("row").filter({ has: page.getByText(MERGED_RULE, { exact: true }) });
  await expect(rows).toHaveCount(1);
  await expect(rows).toContainText("Trivy, osv-scanner");
  await rows.getByRole("button", { name: /^Show / }).click();
  const detail = page.getByRole("heading", { name: "Finding detail" }).locator("xpath=ancestor::*[@data-widget][1]");
  const reporters = detail.locator("[data-reporters] li");
  await expect(reporters).toHaveText(["Trivy 0.76.0: CVE-2026-0001 (high)", "osv-scanner 1.9.2: GHSA-merged-0001 (unrated)"]);
  // OSV-Scanner's row counts it under Trivy's.
  const scanners = page.locator("section").filter({ has: page.getByRole("heading", { name: "Scanners", exact: true }) });
  await expect(scanners.getByRole("row").filter({ hasText: "osv-scanner" })).toContainText("1 counted under another scanner");
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(overflow).toBeLessThanOrEqual(0);
});
