// ConsentDialog (S-673, FR-SC-07): open only while scanners need consent, names
// each of them, says the answer is kept outside the repository, and reports the
// answer — Escape and the close button decline.
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { CONSENT_TEXT } from "../../copy/security.copy.ts";
import { ConsentDialog } from "./ConsentDialog.tsx";

afterEach(cleanup);

function mount(scanners: readonly string[]) {
  const onAccept = vi.fn();
  const onDecline = vi.fn();
  render(<ConsentDialog scanners={scanners} onAccept={onAccept} onDecline={onDecline} />);
  return { onAccept, onDecline };
}

describe("ConsentDialog", () => {
  it("is closed while no scanner needs consent", () => {
    mount([]);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("names every scanner and says the answer is kept in the user-level config, never the repository", () => {
    mount(["semgrep", "trivy", "osv-scanner"]);
    const dialog = screen.getByRole("dialog", { name: CONSENT_TEXT.title });
    const list = within(dialog).getByRole("list", { name: "Scanners that may reach the network" });
    expect(within(list).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["semgrep", "trivy", "osv-scanner"]);
    expect(dialog.textContent).toContain("user-level Logos config, never in the repository");
    expect(dialog.textContent).toContain("later scans do not ask again");
  });

  it("reports an accept", async () => {
    const { onAccept, onDecline } = mount(["bandit"]);
    await userEvent.click(screen.getByRole("button", { name: CONSENT_TEXT.accept }));
    expect(onAccept).toHaveBeenCalledTimes(1);
    expect(onDecline).not.toHaveBeenCalled();
  });

  it("declines on Not now, on Escape and on the close button", async () => {
    const { onAccept, onDecline } = mount(["bandit"]);
    await userEvent.click(screen.getByRole("button", { name: CONSENT_TEXT.decline }));
    await userEvent.keyboard("{Escape}");
    await userEvent.click(screen.getByRole("button", { name: "Close dialog" }));
    expect(onDecline).toHaveBeenCalledTimes(3);
    expect(onAccept).not.toHaveBeenCalled();
  });
});
