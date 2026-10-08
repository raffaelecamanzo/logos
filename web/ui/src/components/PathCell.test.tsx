// PathCell (S-616, FR-UI-44): a long path abbreviates to its first segment, an
// ellipsis and its last two segments; rows that would collide keep more; the
// full path is the title and the accessible name, and the sort key.
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";

import { DataTable } from "./DataTable.tsx";
import { abbreviatePaths, PATH_BUDGET, pathColumn, PathCell } from "./PathCell.tsx";

afterEach(cleanup);

const LONG = "logos-core/src/resolve/scoped/binder.rs"; // 39 chars
const LONGER = "logos-core/src/history/temporal/mining/window/commits.rs";

describe("abbreviatePaths", () => {
  it("abbreviates a path over the budget to first segment, ellipsis, last two", () => {
    expect(LONGER.length).toBeGreaterThan(PATH_BUDGET);
    expect(abbreviatePaths([LONGER]).get(LONGER)).toBe("logos-core/…/window/commits.rs");
  });

  it("renders a path within the budget whole", () => {
    expect(LONG.length).toBeLessThanOrEqual(PATH_BUDGET);
    expect(abbreviatePaths([LONG, "src/main.rs"])).toEqual(
      new Map([
        [LONG, LONG],
        ["src/main.rs", "src/main.rs"],
      ]),
    );
  });

  it("keeps a long path whole when it has no middle segment to drop", () => {
    const flat = "a-very-long-directory-name-for-a-fixture/another-long-one/file.rs";
    expect(flat.length).toBeGreaterThan(PATH_BUDGET);
    expect(abbreviatePaths([flat]).get(flat)).toBe(flat);
  });

  it("gives two paths sharing first and last two segments distinct abbreviations", () => {
    const a = "logos-core/src/history/temporal/mining/window/commits.rs";
    const b = "logos-core/src/history/spatial/mining/window/commits.rs";
    const labels = abbreviatePaths([a, b]);
    expect(labels.get(a)).toBe("logos-core/…/temporal/mining/window/commits.rs");
    expect(labels.get(b)).toBe("logos-core/…/spatial/mining/window/commits.rs");
  });

  it("grows only the colliding rows, and stops at the whole path", () => {
    const a = "web/ui/src/views/analytics/deep/nested/FilesView.tsx";
    const b = "web/ui/src/views/statistics/deep/nested/FilesView.tsx";
    const c = "web/ui/src/components/shared/forms/inputs/TextField.tsx";
    // d differs from a only in its second segment: it collides at every
    // abbreviation, so both end whole rather than looping.
    const d = "web/xx/src/views/analytics/deep/nested/FilesView.tsx";
    const labels = abbreviatePaths([a, b, c, d]);
    expect(labels.get(c)).toBe("web/…/inputs/TextField.tsx");
    expect(labels.get(b)).toBe("web/…/statistics/deep/nested/FilesView.tsx");
    expect(labels.get(a)).toBe(a);
    expect(labels.get(d)).toBe(d);
    expect(new Set(labels.values()).size).toBe(4);
  });
});

describe("PathCell", () => {
  it("shows the abbreviation; the title and the accessible name are the full path", () => {
    render(
      <table>
        <tbody>
          <tr>
            <td>
              <PathCell path={LONGER} label="logos-core/…/window/commits.rs" />
            </td>
          </tr>
        </tbody>
      </table>,
    );
    const cell = screen.getByRole("cell", { name: LONGER });
    const el = within(cell).getByTitle(LONGER);
    expect(el).toHaveAttribute("data-path", LONGER);
    // What a sighted reader sees is the abbreviation; the full path is not
    // rendered twice to assistive technology.
    expect(within(el).getByText("logos-core/…/window/commits.rs")).toHaveAttribute("aria-hidden", "true");
    // Keyboard users reach it, and its full path shows on focus.
    expect(el).toHaveAttribute("tabindex", "0");
  });

  it("renders a short path as itself, unfocusable, with the same title", () => {
    render(
      <table>
        <tbody>
          <tr>
            <td>
              <PathCell path="src/main.rs" />
            </td>
          </tr>
        </tbody>
      </table>,
    );
    const el = screen.getByTitle("src/main.rs");
    expect(el).toHaveTextContent(/^src\/main\.rs$/);
    expect(el).not.toHaveAttribute("tabindex");
    expect(screen.getByRole("cell", { name: "src/main.rs" })).toBeInTheDocument();
  });
});

describe("pathColumn", () => {
  // Full-path order: src/aaa/… < src/b.rs < src/c.rs. Label order differs:
  // "src/…/iii/zzz.rs" sorts after both, because "…" is above every ASCII letter.
  const DEEP = "src/aaa/bbb/ccc/ddd/eee/fff/ggg/hhh/iii/zzz.rs";
  const rows = [{ path: "src/c.rs" }, { path: DEEP }, { path: "src/b.rs" }];

  it("sorts by the full path, not the abbreviation", async () => {
    const user = userEvent.setup();
    render(
      <DataTable
        caption="Files"
        columns={[pathColumn(rows, (r) => r.path)]}
        rows={rows}
        rowKey={(r) => r.path}
      />,
    );
    await user.click(screen.getByRole("button", { name: "File" }));
    const names = screen.getAllByRole("cell").map((c) => c.querySelector("[data-path]")?.getAttribute("data-path"));
    expect(abbreviatePaths([DEEP]).get(DEEP)).toBe("src/…/iii/zzz.rs");
    expect(names).toEqual([DEEP, "src/b.rs", "src/c.rs"]);
  });

  it("abbreviates over the whole table, so a collision on another page still separates", () => {
    const a = "web/ui/src/views/analytics/deep/nested/FilesView.tsx";
    const b = "web/ui/src/views/statistics/deep/nested/FilesView.tsx";
    const table = [{ path: a }, { path: "x.rs" }, { path: b }];
    render(
      <DataTable
        caption="Files"
        columns={[pathColumn(table, (r) => r.path)]}
        rows={table}
        rowKey={(r) => r.path}
        pageSize={2}
      />,
    );
    // Only `a` is on page 1, yet its label already accounts for `b` on page 2.
    expect(screen.getByTitle(a).querySelector('[aria-hidden="true"]')?.textContent).toBe(
      "web/…/analytics/deep/nested/FilesView.tsx",
    );
  });
});
