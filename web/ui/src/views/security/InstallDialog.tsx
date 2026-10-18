/*
 * InstallDialog (S-747, FR-SC-24, FR-SC-22, FR-SC-23) — the one consent dialog
 * before a row's Install or Update, or Install & scan. It extends the scan's
 * consent dialog (`ConsentDialog`, S-673): above the scanners that may reach the
 * network, it names every tool the action downloads, with its version, source
 * and size, and the support installs a tool needs (`uv` and the managed Python,
 * the Java runtime).
 *
 * - Dependency-Check and CodeQL, when Install & scan offers them, are opt-in
 *   lines, unticked: Dependency-Check's line states the Java runtime it brings
 *   and its size; CodeQL's shows its licence terms, and the licence is a line of
 *   its own, unticked. A ticked tool whose licence is not ticked keeps Accept
 *   disabled, so the server is never asked for an install it would refuse.
 * - The network consent lists only the scanners this action scans: an offered
 *   tool joins it when ticked.
 * - Accepting reports exactly what the dialog showed and the user ticked; the
 *   server records consent for those lines alone. Declining starts nothing.
 */

import { useState } from "react";

import type { SecurityConsentLine, SecurityInstallAsk, SecurityInstallPlan } from "../../api/types.ts";
import { Button, Modal } from "../../components/index.ts";
import { CONSENT_TEXT, INSTALL_CONSENT_TEXT } from "../../copy/security.copy.ts";
import styles from "./SecurityView.module.css";

/** The dialog's answers, as the install start sends them. */
export type InstallAnswers = Required<Pick<SecurityInstallAsk, "consent" | "opt_in" | "accept_licences" | "scan_consent">>;

export interface InstallDialogProps {
  /** The plan the server answered; the dialog is open while it is set. */
  plan: SecurityInstallPlan | null;
  /** Install & scan: the network consent is asked too. */
  scan: boolean;
  onAccept: (answers: InstallAnswers) => void;
  onDecline: () => void;
}

type ToolLine = Extract<SecurityConsentLine, { line: "tool" }>;
type SupportLine = Extract<SecurityConsentLine, { line: "support" }>;
type LicenceLine = Extract<SecurityConsentLine, { line: "licence" }>;

export function InstallDialog({ plan, scan, onAccept, onDecline }: InstallDialogProps) {
  // Keyed on the plan, so a new plan starts with every opt-in line unticked.
  return plan === null ? null : <OpenInstallDialog key={JSON.stringify(plan)} plan={plan} scan={scan} onAccept={onAccept} onDecline={onDecline} />;
}

function OpenInstallDialog({ plan, scan, onAccept, onDecline }: InstallDialogProps & { plan: SecurityInstallPlan }) {
  const [ticked, setTicked] = useState<ReadonlySet<string>>(() => new Set());
  const [licences, setLicences] = useState<ReadonlySet<string>>(() => new Set());
  const tools = plan.lines.filter((l): l is ToolLine => l.line === "tool");
  const support = plan.lines.filter((l): l is SupportLine => l.line === "support");
  const licenceLines = plan.lines.filter((l): l is LicenceLine => l.line === "licence");
  const named = tools.filter((t) => !t.offered);
  const offered = tools.filter((t) => t.offered);
  const offeredNames = new Set(offered.map((t) => t.tool));
  const chosen = new Set([...named.map((t) => t.tool), ...ticked]);
  const supportFor = (tools: (tool: string) => boolean) => support.filter((s) => s.for_tools.some(tools));
  const licenceOf = (tool: string) => licenceLines.find((l) => l.tool === tool);
  const scanners = scan ? plan.scan_consent.filter((s) => !offeredNames.has(s) || chosen.has(s)) : [];
  const unlicensed = licenceLines.some((l) => chosen.has(l.tool) && !licences.has(l.tool));

  const toggle = (set: ReadonlySet<string>, name: string, on: boolean) => {
    const next = new Set(set);
    if (on) next.add(name);
    else next.delete(name);
    return next;
  };
  const accept = () =>
    onAccept({
      consent: [
        ...tools.filter((t) => chosen.has(t.tool)).map((t) => `${t.tool} ${t.version}`),
        ...supportFor((t) => chosen.has(t)).map((s) => `${s.tool} ${s.version}`),
      ],
      opt_in: offered.filter((t) => ticked.has(t.tool)).map((t) => t.tool),
      accept_licences: licenceLines.filter((l) => chosen.has(l.tool) && licences.has(l.tool)).map((l) => l.tool),
      scan_consent: scanners,
    });

  const licenceBox = (line: LicenceLine, enabled: boolean) => (
    <div key={`licence:${line.tool}`}>
      <p className="muted">{line.terms}</p>
      <label>
        <input
          type="checkbox"
          checked={licences.has(line.tool)}
          disabled={!enabled}
          onChange={(e) => setLicences((l) => toggle(l, line.tool, e.target.checked))}
        />{" "}
        {INSTALL_CONSENT_TEXT.licence(line.tool)}
      </label>
    </div>
  );

  return (
    <Modal
      open
      onClose={onDecline}
      title={INSTALL_CONSENT_TEXT.title}
      footer={
        <>
          <Button variant="ghost" onClick={onDecline}>
            {INSTALL_CONSENT_TEXT.decline}
          </Button>
          <Button variant="primary" disabled={unlicensed} onClick={accept}>
            {scan ? INSTALL_CONSENT_TEXT.acceptAndScan : INSTALL_CONSENT_TEXT.accept}
          </Button>
        </>
      }
    >
      <p>{INSTALL_CONSENT_TEXT.lead}</p>
      {(named.length > 0 || supportFor((t) => !offeredNames.has(t)).length > 0) && (
        <ul className={styles.list} aria-label={INSTALL_CONSENT_TEXT.tools}>
          {named.map((t) => (
            <li key={`tool:${t.tool}`} className="mono">
              {INSTALL_CONSENT_TEXT.line(t.tool, t.version, t.source, t.size, t.recorded)}
            </li>
          ))}
          {supportFor((t) => !offeredNames.has(t)).map((s) => (
            <li key={`support:${s.tool}`} className="mono">
              {INSTALL_CONSENT_TEXT.support(s.tool, s.version, s.source, s.size, s.for_tools)}
            </li>
          ))}
        </ul>
      )}
      {named.map((t) => licenceOf(t.tool)).flatMap((l) => (l ? [licenceBox(l, true)] : []))}
      {offered.length > 0 && (
        <fieldset aria-label="Opt-in scanners">
          <legend>{INSTALL_CONSENT_TEXT.optIn}</legend>
          {offered.map((t) => {
            const licence = licenceOf(t.tool);
            return (
              <div key={`offered:${t.tool}`}>
                <label className="mono">
                  <input
                    type="checkbox"
                    checked={ticked.has(t.tool)}
                    onChange={(e) => setTicked((s) => toggle(s, t.tool, e.target.checked))}
                  />{" "}
                  {INSTALL_CONSENT_TEXT.line(t.tool, t.version, t.source, t.size, t.recorded)}
                </label>
                {supportFor((name) => name === t.tool).map((s) => (
                  <p key={`support:${s.tool}`} className="muted">
                    {s.tool === "java"
                      ? INSTALL_CONSENT_TEXT.javaRuntime(s.version, s.size)
                      : INSTALL_CONSENT_TEXT.support(s.tool, s.version, s.source, s.size, s.for_tools)}
                  </p>
                ))}
                {licence && licenceBox(licence, ticked.has(t.tool))}
              </div>
            );
          })}
        </fieldset>
      )}
      <p className="muted">{INSTALL_CONSENT_TEXT.where}</p>
      {scanners.length > 0 && (
        <>
          <p>{CONSENT_TEXT.lead}</p>
          <ul className={styles.list} aria-label="Scanners that may reach the network">
            {scanners.map((s) => (
              <li key={s} className="mono">
                {s}
              </li>
            ))}
          </ul>
          <p className="muted">{CONSENT_TEXT.where}</p>
        </>
      )}
    </Modal>
  );
}
