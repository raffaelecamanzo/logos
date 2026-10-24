// The execution-consent dialog (S-739, FR-CV-13): an `EXECUTES CODE` callout says
// the run builds the project and runs its tests; the dialog names each tool that
// will run, shows the custom command exactly as the server escaped it for display
// (never the raw text, whose control characters a page would act on), marks a
// newly enabled tool and a changed command, and Allow / Cancel answer it.
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { COVERAGE_CONSENT_TEXT, COVERAGE_PREPARE_CONSENT_TEXT } from "../../copy/coverageView.copy.ts";
import { CoverageConsentDialog } from "./CoverageConsentDialog.tsx";
import { CONSENT_CHANGED, CONSENT_FIRST, DEPENDENCY_FIRST, INSTALL_FIRST } from "./fixtures/coverageRun.ts";

afterEach(cleanup);

function open(props: Partial<Parameters<typeof CoverageConsentDialog>[0]> = {}) {
  const onAccept = vi.fn();
  const onDecline = vi.fn();
  render(
    <CoverageConsentDialog
      required={CONSENT_FIRST}
      willRun={["vitest", "go", "custom"]}
      consentedBefore={false}
      onAccept={onAccept}
      onDecline={onDecline}
      {...props}
    />,
  );
  return { onAccept, onDecline };
}

const toolNames = () =>
  within(screen.getByRole("list", { name: COVERAGE_CONSENT_TEXT.tools }))
    .getAllByRole("listitem")
    .map((li) => li.textContent);

describe("CoverageConsentDialog (S-739, FR-CV-13)", () => {
  it("is closed while nothing is asked", () => {
    open({ required: null });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("says in an EXECUTES CODE callout that the run executes the project's own code", () => {
    open();
    const dialog = screen.getByRole("dialog", { name: COVERAGE_CONSENT_TEXT.title });
    const callout = within(dialog).getByRole("status");
    expect(callout).toHaveTextContent(COVERAGE_CONSENT_TEXT.label);
    expect(callout).toHaveTextContent(
      "Logos will build this project and run its tests. This runs the project's own code (build scripts, tests) and may download dependencies.",
    );
    expect(dialog).toHaveTextContent("never in the repository");
  });

  it("names each tool that will run and shows the command as the server escaped it", () => {
    open();
    expect(toolNames()).toEqual(["vitest", "go"]);
    const command = screen.getByLabelText(COVERAGE_CONSENT_TEXT.command);
    expect(command.textContent).toBe("make cov\\u{202e}");
    expect(command.textContent).not.toContain("‮");
    // First use: nothing is marked new.
    expect(screen.queryByText(COVERAGE_CONSENT_TEXT.added)).toBeNull();
  });

  it("names a pending tool the page did not list", () => {
    open({ willRun: ["go"] });
    expect(toolNames()).toEqual(["go", "vitest"]);
  });

  it("marks a newly enabled tool once a consent was recorded before", () => {
    const required = { ...CONSENT_FIRST, consent_required: { tools: ["vitest"], refresh_cmd: null, command_changed: false }, refresh_cmd_shown: null };
    open({ required, consentedBefore: true });
    // The command is not asked: the custom row is named as a tool that runs.
    expect(toolNames()).toEqual(["vitest newly enabled", "go", "custom"]);
    expect(screen.queryByLabelText(COVERAGE_CONSENT_TEXT.command)).toBeNull();
  });

  it("marks a changed command and shows its new text", () => {
    open({ required: CONSENT_CHANGED, consentedBefore: true });
    expect(screen.getByLabelText(COVERAGE_CONSENT_TEXT.command).textContent).toBe("make coverage-all");
    expect(screen.getByText(COVERAGE_CONSENT_TEXT.commandChanged)).toBeInTheDocument();
    expect(toolNames()).toEqual(["vitest", "go"]);
  });

  it("answers Allow and run, and Cancel", async () => {
    const { onAccept, onDecline } = open();
    await userEvent.click(screen.getByRole("button", { name: COVERAGE_CONSENT_TEXT.accept }));
    expect(onAccept).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole("button", { name: COVERAGE_CONSENT_TEXT.decline }));
    expect(onDecline).toHaveBeenCalledTimes(1);
  });
});

describe("CoverageConsentDialog before Install and Add dependency (S-750, FR-CV-17, FR-CV-18)", () => {
  it("shows the exact package-manager command and the files it changes, with the scripts warning, before anything changes", async () => {
    const { onAccept, onDecline } = open({ required: DEPENDENCY_FIRST, action: "dependency", willRun: ["vitest"] });
    const dialog = screen.getByRole("dialog", { name: COVERAGE_PREPARE_CONSENT_TEXT.dependencyTitle });
    expect(within(dialog).getByLabelText(COVERAGE_PREPARE_CONSENT_TEXT.command).textContent).toBe("pnpm add -D @vitest/coverage-v8@3");
    expect(dialog).toHaveTextContent("It changes package.json, pnpm-lock.yaml.");
    expect(dialog).toHaveTextContent(COVERAGE_PREPARE_CONSENT_TEXT.scripts);
    expect(dialog).toHaveTextContent("Logos never commits");
    // The execution consent is asked in the same dialog, for the tool the addition enables.
    expect(toolNames()).toEqual(["vitest"]);
    expect(within(dialog).getByText(COVERAGE_CONSENT_TEXT.label)).toBeInTheDocument();
    expect(onAccept).not.toHaveBeenCalled();
    await userEvent.click(within(dialog).getByRole("button", { name: COVERAGE_PREPARE_CONSENT_TEXT.acceptDependency }));
    expect(onAccept).toHaveBeenCalledTimes(1);
    expect(onDecline).not.toHaveBeenCalled();
  });

  it("names each download and the rustup step before an Install", async () => {
    const { onDecline } = open({ required: INSTALL_FIRST, action: "install", willRun: ["cargo-llvm-cov"] });
    const dialog = screen.getByRole("dialog", { name: COVERAGE_PREPARE_CONSENT_TEXT.installTitle });
    const lines = within(within(dialog).getByRole("list", { name: COVERAGE_PREPARE_CONSENT_TEXT.installLines }))
      .getAllByRole("listitem")
      .map((li) => li.textContent);
    expect(lines).toEqual([
      "cargo-llvm-cov 0.9.1 from github.com (1.5 MB)",
      "`rustup component add llvm-tools-preview` on your active Rust toolchain, run only if the component is missing",
    ]);
    expect(dialog).toHaveTextContent("SHA-256");
    expect(within(dialog).queryByLabelText(COVERAGE_PREPARE_CONSENT_TEXT.command)).toBeNull();
    await userEvent.click(within(dialog).getByRole("button", { name: COVERAGE_CONSENT_TEXT.decline }));
    expect(onDecline).toHaveBeenCalledTimes(1);
  });

  it("keeps the run's own title and Accept for a plain run", () => {
    open();
    expect(screen.getByRole("dialog", { name: COVERAGE_CONSENT_TEXT.title })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: COVERAGE_CONSENT_TEXT.accept })).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: COVERAGE_PREPARE_CONSENT_TEXT.installLines })).toBeNull();
  });
});
