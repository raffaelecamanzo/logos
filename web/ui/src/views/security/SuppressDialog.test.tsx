// SuppressDialog (S-672, FR-SC-12): an empty reason is refused with no write
// attempted, the dialog names the committed file it writes, the reason is sent
// trimmed with the optional expiry, and a server refusal is stated verbatim with
// the dialog left open.
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { SecurityRequestError } from "../../api/securityClient.ts";
import type { SecurityFinding, SecurityModel } from "../../api/types.ts";
import { IGNORE_FILE, SUPPRESS_TEXT } from "../../copy/security.copy.ts";
import real from "./fixtures/security.real.json";
import { SuppressDialog } from "./SuppressDialog.tsx";

afterEach(cleanup);

const SQL = (real as unknown as SecurityModel).security.findings.findings.find(
  (f) => f.rule_id === "rust.lang.security.formatted-sql",
) as SecurityFinding;

function mount(onSubmit = vi.fn((_reason: string, _expires: string | null) => Promise.resolve())) {
  const onCancel = vi.fn();
  render(<SuppressDialog finding={SQL} onSubmit={onSubmit} onCancel={onCancel} />);
  return { onSubmit, onCancel, dialog: screen.getByRole("dialog", { name: SUPPRESS_TEXT.title }) };
}

describe("SuppressDialog", () => {
  it("names the finding and the committed file it writes", () => {
    const { dialog } = mount();
    expect(IGNORE_FILE).toBe(".logos/security-ignore.toml");
    expect(dialog.textContent).toContain(".logos/security-ignore.toml");
    expect(dialog.textContent).toContain("rust.lang.security.formatted-sql at src/lib.rs:3");
  });

  it("refuses an empty reason and sends nothing", async () => {
    const { onSubmit, dialog } = mount();
    await userEvent.click(within(dialog).getByRole("button", { name: SUPPRESS_TEXT.confirm }));
    expect(onSubmit).not.toHaveBeenCalled();
    const reason = within(dialog).getByRole("textbox", { name: SUPPRESS_TEXT.reason });
    expect(reason).toHaveAttribute("aria-invalid", "true");
    expect(within(dialog).getByRole("alert").textContent).toBe(SUPPRESS_TEXT.reasonRequired);

    // Whitespace is no reason either.
    await userEvent.type(reason, "   ");
    await userEvent.click(within(dialog).getByRole("button", { name: SUPPRESS_TEXT.confirm }));
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("sends the trimmed reason, and no expiry when none is given", async () => {
    const { onSubmit, dialog } = mount();
    await userEvent.type(within(dialog).getByRole("textbox", { name: SUPPRESS_TEXT.reason }), "  built from a constant ");
    await userEvent.click(within(dialog).getByRole("button", { name: SUPPRESS_TEXT.confirm }));
    expect(onSubmit).toHaveBeenCalledWith("built from a constant", null);
  });

  it("sends the expiry as a calendar date when one is given", async () => {
    const { onSubmit, dialog } = mount();
    await userEvent.type(within(dialog).getByRole("textbox", { name: SUPPRESS_TEXT.reason }), "vendored");
    fireEvent.change(within(dialog).getByLabelText(SUPPRESS_TEXT.expires), { target: { value: "2027-01-31" } });
    await userEvent.click(within(dialog).getByRole("button", { name: SUPPRESS_TEXT.confirm }));
    expect(onSubmit).toHaveBeenCalledWith("vendored", "2027-01-31");
  });

  it("states the server's refusal and stays open", async () => {
    const refusal = new SecurityRequestError(422, "invalid value for .logos/security-ignore.toml: `expires` 2020-01-01 is already past");
    const { onCancel, dialog } = mount(vi.fn(() => Promise.reject(refusal)));
    await userEvent.type(within(dialog).getByRole("textbox", { name: SUPPRESS_TEXT.reason }), "r");
    await userEvent.click(within(dialog).getByRole("button", { name: SUPPRESS_TEXT.confirm }));
    const alert = await within(dialog).findByRole("alert");
    expect(alert.textContent).toBe(SUPPRESS_TEXT.saveFailed(refusal.detail));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(onCancel).not.toHaveBeenCalled();
    // The user can correct the entry and try again.
    expect(within(dialog).getByRole("button", { name: SUPPRESS_TEXT.confirm })).toBeEnabled();
  });

  it("sends one write while it is in flight: Confirm is disabled until it settles", async () => {
    const onSubmit = vi.fn((_reason: string, _expires: string | null) => new Promise<void>(() => {}));
    const { dialog } = mount(onSubmit);
    await userEvent.type(within(dialog).getByRole("textbox", { name: SUPPRESS_TEXT.reason }), "r");
    const confirm = within(dialog).getByRole("button", { name: SUPPRESS_TEXT.confirm });
    await userEvent.click(confirm);
    expect(confirm).toBeDisabled();
    await userEvent.click(confirm);
    expect(onSubmit).toHaveBeenCalledTimes(1);
  });

  it("cancels on Cancel and on Escape", async () => {
    const { onCancel, onSubmit, dialog } = mount();
    await userEvent.click(within(dialog).getByRole("button", { name: SUPPRESS_TEXT.cancel }));
    await userEvent.keyboard("{Escape}");
    expect(onCancel).toHaveBeenCalledTimes(2);
    expect(onSubmit).not.toHaveBeenCalled();
  });
});
