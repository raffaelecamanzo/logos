import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { QueryResponse } from "../../api/types.ts";
import { GraphQuery } from "./GraphQuery.tsx";

/** A `callers-of` answer: `by_dyn` reaches the target only through `Tr::m`, `by_impl` calls it directly. */
const CALLERS: QueryResponse = {
  mode: "relation",
  query: "callers of m",
  hits: [
    { id: "scip::by_dyn", label: "by_dyn", kind: "function", layer: "code", file: "a/src/lib.rs", line: 5, rank: 1, via: "Tr::m" },
    { id: "scip::by_impl", label: "by_impl", kind: "function", layer: "code", file: "a/src/lib.rs", line: 6, rank: 2 },
  ],
  total: 2,
  note: null,
  suggestions: [],
  warnings: [],
};

afterEach(() => {
  cleanup(); // RTL's auto-cleanup is off under globals:false — unmount explicitly
  vi.unstubAllGlobals();
});

describe("relational query hits (S-712)", () => {
  it("labels a hit reached through a trait fan-out with its trait method, and no other", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve({ ok: true, json: () => Promise.resolve(CALLERS) } as Response)),
    );
    render(<GraphQuery onSelect={() => {}} />);
    await userEvent.selectOptions(screen.getByLabelText("Relation"), "callers-of");
    await userEvent.type(screen.getByLabelText("Target"), "m");
    await userEvent.click(screen.getByRole("button", { name: "Query" }));

    const results = screen.getByRole("region", { name: "Query results" });
    const row = async (name: string) => (await within(results).findByRole("button", { name })).closest("tr")!;
    expect(within(await row("by_dyn")).getByText("via Tr::m")).toBeInTheDocument();
    expect(within(await row("by_impl")).queryByText(/via/)).not.toBeInTheDocument();
  });
});
