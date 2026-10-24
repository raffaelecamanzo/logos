/*
 * CoverageConsentDialog (S-739, FR-CV-13, FR-CV-15) — the execution consent asked
 * before a coverage run from the Coverage page, modelled on the scan's consent
 * dialog (`security/ConsentDialog.tsx`). An `EXECUTES CODE` callout says in words
 * that Logos builds the project and runs its tests, which runs the project's own
 * code and may download dependencies; the dialog names each tool that will run
 * and shows the custom command when it is pending.
 *
 * What it shows comes from the server's `409`: the pending tools, and the custom
 * command escaped as the terminal consent shows it (`refresh_cmd_shown`), so the
 * command a user reads is the command that runs. A tool the recorded consent does
 * not name yet is marked when another was allowed before, and a changed command
 * is marked as changed. Accepting starts the run with exactly what was shown;
 * declining starts and records nothing.
 *
 * S-750 (FR-CV-17, FR-CV-18): before an Install the dialog also names each
 * download — version, source and size, as `logos coverage install` names it —
 * and the `rustup component add llvm-tools-preview` step; before an Add
 * dependency, the exact package-manager command, the files it changes, that the
 * package manager may run lifecycle scripts, and that Logos never commits. One
 * Accept answers every line; nothing changes before it.
 */

import { Badge, Button, Callout, Modal } from "../../components/index.ts";
import type { CoverageConsentRequired, CoveragePrepareRequired } from "../../api/index.ts";
import { COVERAGE_CONSENT_TEXT, COVERAGE_PREPARE_CONSENT_TEXT } from "../../copy/coverageView.copy.ts";
import { CUSTOM_TOOL } from "./coverageRunModel.ts";
import styles from "./AnalyticsView.module.css";

export interface CoverageConsentDialogProps {
  /** What the consent still lacks; the dialog is open while it is set. */
  required: CoverageConsentRequired | CoveragePrepareRequired | null;
  /** The start it answers: a run, or one that installs or adds first. */
  action?: "run" | "install" | "dependency";
  /** The tools a run starts here (registry names, `custom` for the command). */
  willRun: readonly string[];
  /** Whether the project's consent named any tool before: then a pending tool is
   *  a newly enabled one, and is marked so. */
  consentedBefore: boolean;
  onAccept: () => void;
  onDecline: () => void;
}

export function CoverageConsentDialog({
  required,
  action = "run",
  willRun,
  consentedBefore,
  onAccept,
  onDecline,
}: CoverageConsentDialogProps) {
  const pending = required?.consent_required;
  const install = required !== null && "install" in required ? required.install : [];
  const dependencies = required !== null && "dependencies" in required ? required.dependencies : [];
  const title =
    action === "install"
      ? COVERAGE_PREPARE_CONSENT_TEXT.installTitle
      : action === "dependency"
        ? COVERAGE_PREPARE_CONSENT_TEXT.dependencyTitle
        : COVERAGE_CONSENT_TEXT.title;
  const acceptLabel =
    action === "install"
      ? COVERAGE_PREPARE_CONSENT_TEXT.acceptInstall
      : action === "dependency"
        ? COVERAGE_PREPARE_CONSENT_TEXT.acceptDependency
        : COVERAGE_CONSENT_TEXT.accept;
  const pendingTools = pending?.tools ?? [];
  const shown = required?.refresh_cmd_shown ?? null;
  // Every tool that will run, then any pending one this page did not list; the
  // custom command is named by its text below when that is what is asked.
  const tools = [...willRun, ...pendingTools.filter((t) => !willRun.includes(t))].filter(
    (t) => t !== CUSTOM_TOOL || shown === null,
  );
  return (
    <Modal
      open={required !== null}
      onClose={onDecline}
      title={title}
      footer={
        <>
          <Button variant="ghost" onClick={onDecline}>
            {COVERAGE_CONSENT_TEXT.decline}
          </Button>
          <Button variant="primary" onClick={onAccept}>
            {acceptLabel}
          </Button>
        </>
      }
    >
      {install.length > 0 && (
        <>
          <p>{COVERAGE_PREPARE_CONSENT_TEXT.installLead}</p>
          <ul className={styles.toolList} aria-label={COVERAGE_PREPARE_CONSENT_TEXT.installLines}>
            {install.map((line) => (
              <li key={line.line === "tool" ? line.tool : line.command} className="mono">
                {line.text}
              </li>
            ))}
          </ul>
          <p className="muted">{COVERAGE_PREPARE_CONSENT_TEXT.installWhere}</p>
        </>
      )}
      {dependencies.map((dependency) => (
        <div key={dependency.command}>
          <p>{COVERAGE_PREPARE_CONSENT_TEXT.dependencyLead}</p>
          <pre className={`mono ${styles.command}`} aria-label={COVERAGE_PREPARE_CONSENT_TEXT.command}>
            {dependency.command}
          </pre>
          <p>{COVERAGE_PREPARE_CONSENT_TEXT.files(dependency.files)}</p>
          <Callout label={COVERAGE_PREPARE_CONSENT_TEXT.scriptsLabel} tone="signal">
            {COVERAGE_PREPARE_CONSENT_TEXT.scripts}
          </Callout>
          <p className="muted">{COVERAGE_PREPARE_CONSENT_TEXT.dependencyWhere}</p>
        </div>
      ))}
      <Callout label={COVERAGE_CONSENT_TEXT.label} tone="signal">
        {COVERAGE_CONSENT_TEXT.lead}
      </Callout>
      <ul className={styles.toolList} aria-label={COVERAGE_CONSENT_TEXT.tools}>
        {tools.map((tool) => (
          <li key={tool}>
            <span className="mono">{tool}</span>
            {consentedBefore && pendingTools.includes(tool) && (
              <>
                {" "}
                <Badge tone="orange">{COVERAGE_CONSENT_TEXT.added}</Badge>
              </>
            )}
          </li>
        ))}
      </ul>
      {shown !== null && (
        <>
          <p>{COVERAGE_CONSENT_TEXT.command}</p>
          <pre className={`mono ${styles.command}`} aria-label={COVERAGE_CONSENT_TEXT.command}>
            {shown}
          </pre>
          {pending?.command_changed && (
            <p>
              <Badge tone="orange">{COVERAGE_CONSENT_TEXT.commandChanged}</Badge>
            </p>
          )}
        </>
      )}
      <p className="muted">{COVERAGE_CONSENT_TEXT.where}</p>
    </Modal>
  );
}
