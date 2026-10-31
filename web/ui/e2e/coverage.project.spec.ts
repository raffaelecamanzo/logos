// The Coverage tools table carries the run, in a real browser (S-803, CR-250,
// frontend-design §4.10, FR-CV-15): a run of the fixture's one coverage tool — the
// `custom` entry, a `refresh_cmd` that prints a failing test and exits non-zero
// (serve-fixture.sh) — runs from the page after its consent, and ends failed. The
// row reads `failed` with its recorded Elapsed, and the message area under the table
// names the failing test, the cause S-802 records, and the log path; the State cell
// carries no log path. Read again after a reload, the page shows the same: the
// duration and the cause come from the stored last-run record, not from the run the
// page followed.
import { expect, test, type Locator, type Page } from "@playwright/test";

import { COVERAGE_CONSENT_TEXT, COVERAGE_RUN_TEXT } from "../src/copy/coverageView.copy.ts";

const FAILING_TEST = "tests::checkout_totals_include_tax";
const LOG = ".logos/coverage/custom.failed.log";

/** A recorded duration in the table's words: under a second, or whole seconds. */
const DURATION = /^(<1 s|\d+ s|\d+ min \d+ s)$/;

/** The `custom` row's State and Elapsed cells, by column header. */
async function customRow(page: Page): Promise<{ row: Locator; state: Locator; elapsed: Locator }> {
  const table = page.getByRole("table", { name: "Coverage tools" });
  await expect(table).toBeVisible();
  const headers = (await table.getByRole("columnheader").allTextContents()).map((h) => h.replace(/[↑↓↕]/g, "").trim());
  expect(headers, "Elapsed sits between State and Last run").toEqual(["Tool", "Version", "State", "Elapsed", "Last run", "Files"]);
  const row = table.getByRole("row").filter({ has: page.getByRole("cell", { name: "custom", exact: true }) });
  return { row, state: row.getByRole("cell").nth(2), elapsed: row.getByRole("cell").nth(3) };
}

/** The page shows the failed run: State, recorded Elapsed, and the cause below. */
async function expectFailedRun(page: Page) {
  const { state, elapsed } = await customRow(page);
  await expect(state).toHaveText("failed");
  await expect(state, "the log path is the message area's, not the State cell's").not.toContainText(LOG);
  await expect(elapsed, "a recorded duration, never the em dash").toHaveText(DURATION);

  const messages = page.getByRole("list", { name: COVERAGE_RUN_TEXT.messages });
  await expect(messages).toHaveCount(1);
  const entry = messages.getByRole("listitem");
  await expect(entry).toHaveCount(1);
  await expect(entry).toContainText("custom");
  await expect(entry, "the cause names the failing test").toContainText(FAILING_TEST);
  await expect(entry).toContainText(COVERAGE_RUN_TEXT.log(LOG));
  // The table first, the cause below it.
  const table = await page.getByRole("table", { name: "Coverage tools" }).boundingBox();
  const area = await messages.boundingBox();
  expect(table !== null && area !== null && area.y >= table.y + table.height, "the message area sits below the table").toBe(true);
}

test("a failed run's cause names the failing test below the table, with its Elapsed, and is read again after a reload", async ({ page }) => {
  test.setTimeout(60_000);
  await page.goto("/coverage");
  const widget = page.locator("[data-widget]").filter({ has: page.getByRole("heading", { name: "Coverage tools" }) });

  // Before the run: the tool never ran, so no duration and nothing below the table.
  const before = await customRow(page);
  await expect(before.state).toHaveText("never run");
  await expect(before.elapsed).toHaveText("—");
  await expect(page.getByRole("list", { name: COVERAGE_RUN_TEXT.messages })).toHaveCount(0);

  // Run coverage asks for the execution consent first, and starts what it showed.
  await widget.getByRole("button", { name: COVERAGE_RUN_TEXT.run }).click();
  const dialog = page.getByRole("dialog", { name: COVERAGE_CONSENT_TEXT.title });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: COVERAGE_CONSENT_TEXT.accept }).click();

  // There is no progress list: the run is the table's.
  await expect(page.getByRole("list", { name: "Coverage run progress" })).toHaveCount(0);
  await expect(page.getByRole("list", { name: COVERAGE_RUN_TEXT.messages })).toBeVisible({ timeout: 30_000 });
  await expectFailedRun(page);
  await expect(widget.getByRole("button", { name: COVERAGE_RUN_TEXT.run }), "the run is shown ended").toBeVisible();

  // Read again after a reload: the stored record carries both.
  await page.reload();
  await expectFailedRun(page);
  await expect(page.getByRole("list", { name: "Coverage run progress" })).toHaveCount(0);
});
