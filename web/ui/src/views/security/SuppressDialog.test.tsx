// SuppressDialog (S-672, FR-SC-12): an empty reason is refused with no write
// attempted, the dialog names the committed file it writes, the reason is sent
// trimmed with the optional expiry, and a server refusal is stated verbatim with
// the dialog left open. "Everything under this path" (S-770) sends a path
// selector starting from the finding's directory, narrowed on request.
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { SecurityRequestError } from "../../api/securityClient.ts";
import type { SecurityFinding, SecurityModel, SecuritySelector } from "../../api/types.ts";
import { IGNORE_FILE, SUPPRESS_TEXT } from "../../copy/security.copy.ts";
import real from "./fixtures/security.real.json";
import { SuppressDialog } from "./SuppressDialog.tsx";

afterEach(cleanup);

const SQL = (real as unknown as SecurityModel).security.findings.findings.find(
  (f) => f.rule_id === "rust.lang.security.formatted-sql",
) as SecurityFinding;

type Submit = (selector: SecuritySelector, reason: string, expires: string | null) => Promise<void>;

function mount(onSubmit = vi.fn<Submit>(() => Promise.resolve())) {
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
    expect(onSubmit).toHaveBeenCalledWith({ fingerprint: SQL.fingerprint }, "built from a constant", null);
  });

  it("sends the expiry as a calendar date when one is given", async () => {
    const { onSubmit, dialog } = mount();
    await userEvent.type(within(dialog).getByRole("textbox", { name: SUPPRESS_TEXT.reason }), "vendored");
    fireEvent.change(within(dialog).getByLabelText(SUPPRESS_TEXT.expires), { target: { value: "2027-01-31" } });
    await userEvent.click(within(dialog).getByRole("button", { name: SUPPRESS_TEXT.confirm }));
    expect(onSubmit).toHaveBeenCalledWith({ fingerprint: SQL.fingerprint }, "vendored", "2027-01-31");
  });

  it("states the server's refusal and stays open", async () => {
    const refusal = new SecurityRequestError(422, "invalid value for .logos/security-ignore.toml: `expires` 2020-01-01 is already past");
    const { onCancel, dialog } = mount(vi.fn<Submit>(() => Promise.reject(refusal)));
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
    const onSubmit = vi.fn<Submit>(() => new Promise<void>(() => {}));
    const { dialog } = mount(onSubmit);
    await userEvent.type(within(dialog).getByRole("textbox", { name: SUPPRESS_TEXT.reason }), "r");
    const confirm = within(dialog).getByRole("button", { name: SUPPRESS_TEXT.confirm });
    await userEvent.click(confirm);
    expect(confirm).toBeDisabled();
    await userEvent.click(confirm);
    expect(onSubmit).toHaveBeenCalledTimes(1);
  });

  it("offers everything under the finding's directory, and sends that path", async () => {
    const { onSubmit, dialog } = mount();
    expect(within(dialog).queryByRole("textbox", { name: SUPPRESS_TEXT.path })).toBeNull();
    await userEvent.click(within(dialog).getByRole("radio", { name: SUPPRESS_TEXT.scopePath }));
    const path = within(dialog).getByRole("textbox", { name: SUPPRESS_TEXT.path });
    expect(path).toHaveValue("src");
    await userEvent.type(within(dialog).getByRole("textbox", { name: SUPPRESS_TEXT.reason }), "generated");
    await userEvent.click(within(dialog).getByRole("button", { name: SUPPRESS_TEXT.confirm }));
    expect(onSubmit).toHaveBeenCalledWith({ path: "src" }, "generated", null);
  });

  it("narrows the path entry to the finding's rule and scanner when ticked, and sends an edited path", async () => {
    const { onSubmit, dialog } = mount();
    await userEvent.click(within(dialog).getByRole("radio", { name: SUPPRESS_TEXT.scopePath }));
    const path = within(dialog).getByRole("textbox", { name: SUPPRESS_TEXT.path });
    await userEvent.clear(path);
    await userEvent.type(path, " src/generated/ ");
    await userEvent.click(within(dialog).getByRole("checkbox", { name: SUPPRESS_TEXT.onlyRule(SQL.rule_id) }));
    await userEvent.click(within(dialog).getByRole("checkbox", { name: SUPPRESS_TEXT.onlyScanner(SQL.scanner_name) }));
    await userEvent.type(within(dialog).getByRole("textbox", { name: SUPPRESS_TEXT.reason }), "r");
    await userEvent.click(within(dialog).getByRole("button", { name: SUPPRESS_TEXT.confirm }));
    // The server writes the canonical spelling; the dialog sends the path trimmed.
    expect(onSubmit).toHaveBeenCalledWith({ path: "src/generated/", rule: SQL.rule_id, scanner: SQL.scanner_name }, "r", null);
  });

  it("refuses an empty path and sends nothing, and going back to one finding sends its fingerprint", async () => {
    const { onSubmit, dialog } = mount();
    await userEvent.click(within(dialog).getByRole("radio", { name: SUPPRESS_TEXT.scopePath }));
    await userEvent.clear(within(dialog).getByRole("textbox", { name: SUPPRESS_TEXT.path }));
    await userEvent.type(within(dialog).getByRole("textbox", { name: SUPPRESS_TEXT.reason }), "r");
    await userEvent.click(within(dialog).getByRole("button", { name: SUPPRESS_TEXT.confirm }));
    expect(onSubmit).not.toHaveBeenCalled();
    expect(within(dialog).getByRole("alert").textContent).toBe(SUPPRESS_TEXT.pathRequired);

    await userEvent.click(within(dialog).getByRole("radio", { name: SUPPRESS_TEXT.scopeFinding }));
    await userEvent.click(within(dialog).getByRole("button", { name: SUPPRESS_TEXT.confirm }));
    expect(onSubmit).toHaveBeenCalledWith({ fingerprint: SQL.fingerprint }, "r", null);
  });

  it("cancels on Cancel and on Escape", async () => {
    const { onCancel, onSubmit, dialog } = mount();
    await userEvent.click(within(dialog).getByRole("button", { name: SUPPRESS_TEXT.cancel }));
    await userEvent.keyboard("{Escape}");
    expect(onCancel).toHaveBeenCalledTimes(2);
    expect(onSubmit).not.toHaveBeenCalled();
  });
});
