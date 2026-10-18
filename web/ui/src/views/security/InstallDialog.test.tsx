// InstallDialog (S-747, FR-SC-24, FR-SC-23) over the real plan
// `web/tests/security_install_job.rs` received for Install & scan on a machine
// with no scanner: every default tool is named with its version, source and size;
// Dependency-Check and CodeQL are opt-in lines, unticked, with the Java runtime's
// size and the CodeQL licence text, the licence a checkbox of its own; accepting
// sends exactly what was shown and ticked, and nothing for an unticked line.
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { SecurityInstallPlan } from "../../api/types.ts";
import { CONSENT_TEXT, INSTALL_CONSENT_TEXT, megabytes } from "../../copy/security.copy.ts";
import real from "./fixtures/install-plan.real.json";
import { InstallDialog } from "./InstallDialog.tsx";

const PLAN = real as unknown as SecurityInstallPlan;
const DEFAULTS = ["trivy", "gitleaks", "osv-scanner", "gosec"];

afterEach(cleanup);

function mount(plan: SecurityInstallPlan | null, scan = true) {
  const onAccept = vi.fn();
  const onDecline = vi.fn();
  render(<InstallDialog plan={plan} scan={scan} onAccept={onAccept} onDecline={onDecline} />);
  return { onAccept, onDecline };
}

const line = (tool: string) => PLAN.lines.find((l) => l.tool === tool && l.line !== "licence")!;
const checkbox = (name: RegExp) => screen.getByRole("checkbox", { name });

describe("InstallDialog", () => {
  it("is closed without a plan", () => {
    mount(null);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("names each default tool with its version, source and size", () => {
    mount(PLAN);
    const dialog = screen.getByRole("dialog", { name: INSTALL_CONSENT_TEXT.title });
    const listed = within(within(dialog).getByRole("list", { name: INSTALL_CONSENT_TEXT.tools })).getAllByRole("listitem");
    expect(listed.map((li) => li.textContent)).toEqual(
      DEFAULTS.map((tool) => {
        const l = line(tool) as Extract<SecurityInstallPlan["lines"][number], { line: "tool" }>;
        return `${tool} ${l.version} — fixtures.invalid, ${megabytes(l.size)}`;
      }),
    );
    expect(dialog.textContent).toContain("never in the repository");
  });

  it("lists Dependency-Check and CodeQL as opt-in lines, unticked, with the Java runtime's size and the CodeQL licence text", () => {
    mount(PLAN);
    const optIn = screen.getByRole("group", { name: "Opt-in scanners" });
    const dc = within(optIn).getByRole("checkbox", { name: /^dependency-check 13\.0\.0/ });
    const codeql = within(optIn).getByRole("checkbox", { name: /^codeql 2\.27\.2/ });
    expect([(dc as HTMLInputElement).checked, (codeql as HTMLInputElement).checked]).toEqual([false, false]);
    const java = line("java") as Extract<SecurityInstallPlan["lines"][number], { line: "support" }>;
    expect(optIn.textContent).toContain(INSTALL_CONSENT_TEXT.javaRuntime(java.version, java.size));
    expect(optIn.textContent).toContain(`${megabytes(java.size)}`);
    const terms = PLAN.lines.find((l) => l.line === "licence")!;
    expect(terms.line === "licence" && optIn.textContent?.includes(terms.terms)).toBe(true);
    const licence = checkbox(new RegExp(INSTALL_CONSENT_TEXT.licence("codeql")));
    expect((licence as HTMLInputElement).checked, "the licence is its own line, unticked").toBe(false);
    expect((licence as HTMLInputElement).disabled, "it is answered once CodeQL is ticked").toBe(true);
  });

  it("accepts the default lines and their scanners only, while the opt-in lines stay unticked", async () => {
    const { onAccept } = mount(PLAN);
    const scanners = screen.getByRole("list", { name: "Scanners that may reach the network" });
    expect(within(scanners).getAllByRole("listitem").map((li) => li.textContent)).toEqual(DEFAULTS);
    expect(screen.getByRole("dialog").textContent).toContain(CONSENT_TEXT.where);
    await userEvent.click(screen.getByRole("button", { name: INSTALL_CONSENT_TEXT.acceptAndScan }));
    expect(onAccept).toHaveBeenCalledWith({
      consent: ["trivy 0.74.0", "gitleaks 8.18.4", "osv-scanner 1.9.2", "gosec 2.21.4"],
      opt_in: [],
      accept_licences: [],
      scan_consent: DEFAULTS,
    });
  });

  it("adds a ticked Dependency-Check with its Java runtime and its network consent", async () => {
    const { onAccept } = mount(PLAN);
    await userEvent.click(checkbox(/^dependency-check 13\.0\.0/));
    await userEvent.click(screen.getByRole("button", { name: INSTALL_CONSENT_TEXT.acceptAndScan }));
    expect(onAccept).toHaveBeenCalledWith({
      consent: ["trivy 0.74.0", "gitleaks 8.18.4", "osv-scanner 1.9.2", "gosec 2.21.4", "dependency-check 13.0.0", "java 21.0.12.1"],
      opt_in: ["dependency-check"],
      accept_licences: [],
      scan_consent: [...DEFAULTS, "dependency-check"],
    });
  });

  it("keeps Accept disabled for a ticked CodeQL until its licence line is ticked too", async () => {
    const { onAccept } = mount(PLAN);
    const accept = screen.getByRole("button", { name: INSTALL_CONSENT_TEXT.acceptAndScan });
    await userEvent.click(checkbox(/^codeql 2\.27\.2/));
    expect(accept).toBeDisabled();
    await userEvent.click(checkbox(new RegExp(INSTALL_CONSENT_TEXT.licence("codeql"))));
    expect(accept).toBeEnabled();
    await userEvent.click(accept);
    expect(onAccept.mock.calls[0][0]).toMatchObject({ opt_in: ["codeql"], accept_licences: ["codeql"] });
    expect(onAccept.mock.calls[0][0].consent).toContain("codeql 2.27.2");
  });

  it("asks a row's CodeQL install for its licence before Install is enabled, and no network consent", async () => {
    const plan: SecurityInstallPlan = {
      lines: PLAN.lines.filter((l) => l.tool === "codeql").map((l) => (l.line === "tool" ? { ...l, offered: false } : l)),
      kept: [],
      scan_consent: [],
    };
    const { onAccept } = mount(plan, false);
    expect(screen.queryByRole("group", { name: "Opt-in scanners" })).toBeNull();
    expect(screen.queryByRole("list", { name: "Scanners that may reach the network" })).toBeNull();
    const install = screen.getByRole("button", { name: INSTALL_CONSENT_TEXT.accept });
    expect(install).toBeDisabled();
    await userEvent.click(checkbox(new RegExp(INSTALL_CONSENT_TEXT.licence("codeql"))));
    await userEvent.click(install);
    expect(onAccept).toHaveBeenCalledWith({ consent: ["codeql 2.27.2"], opt_in: [], accept_licences: ["codeql"], scan_consent: [] });
  });

  it("lists the support installs a named Python scanner needs, and accepts exactly the lines it showed", async () => {
    const plan: SecurityInstallPlan = {
      lines: [
        { line: "tool", tool: "semgrep", version: "1.95.0", source: "pypi.org", size: 40_000_000, recorded: false, offered: false },
        { line: "support", tool: "uv", version: "0.5.11", source: "github.com", size: 16_000_000, recorded: false, for_tools: ["semgrep"] },
        { line: "support", tool: "python", version: "3.12.8", source: "github.com", size: 21_000_000, recorded: true, for_tools: ["semgrep"] },
      ],
      kept: [],
      scan_consent: [],
    };
    const { onAccept } = mount(plan, false);
    const listed = within(screen.getByRole("list", { name: INSTALL_CONSENT_TEXT.tools })).getAllByRole("listitem").map((li) => li.textContent);
    expect(listed).toEqual([
      INSTALL_CONSENT_TEXT.line("semgrep", "1.95.0", "pypi.org", 40_000_000, false),
      INSTALL_CONSENT_TEXT.support("uv", "0.5.11", "github.com", 16_000_000, ["semgrep"]),
      INSTALL_CONSENT_TEXT.support("python", "3.12.8", "github.com", 21_000_000, ["semgrep"]),
    ]);
    await userEvent.click(screen.getByRole("button", { name: INSTALL_CONSENT_TEXT.accept }));
    expect(onAccept).toHaveBeenCalledWith({ consent: ["semgrep 1.95.0", "uv 0.5.11", "python 3.12.8"], opt_in: [], accept_licences: [], scan_consent: [] });
  });

  it("declines on Not now and on Escape", async () => {
    const { onAccept, onDecline } = mount(PLAN);
    await userEvent.click(screen.getByRole("button", { name: INSTALL_CONSENT_TEXT.decline }));
    await userEvent.keyboard("{Escape}");
    expect(onDecline).toHaveBeenCalledTimes(2);
    expect(onAccept).not.toHaveBeenCalled();
  });
});
