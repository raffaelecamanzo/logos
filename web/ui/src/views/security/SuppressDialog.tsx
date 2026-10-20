/*
 * SuppressDialog (S-672, FR-SC-12) — accept one finding's risk from the
 * Security page. A reason is required and an expiry optional; the dialog names
 * the committed file the suppression is written to (`.logos/security-ignore.toml`)
 * before anything is written. An empty reason is refused here, with no request
 * sent; a refusal from the server (a past expiry, a hand-spelled file, a glob
 * covering the whole project) is shown verbatim and the dialog stays open.
 *
 * "Everything under this path" (S-770) writes a `path` entry instead of the
 * finding's fingerprint: the path starts as the finding's directory and can be
 * edited, and the entry can be narrowed to the finding's rule and scanner. The
 * server writes the path in its canonical spelling.
 */

import { useState } from "react";

import { requestDetail } from "../../api/securityClient.ts";
import type { SecurityFinding, SecuritySelector } from "../../api/types.ts";
import { Button, Modal, TextareaField, TextField } from "../../components/index.ts";
import { SUPPRESS_TEXT } from "../../copy/security.copy.ts";
import { findingLocation, suggestedPath } from "./securityModel.ts";
import styles from "./SecurityView.module.css";

export interface SuppressDialogProps {
  /** The finding to suppress. */
  finding: SecurityFinding;
  /** Writes the suppression; rejects with the reason it was not written. */
  onSubmit: (selector: SecuritySelector, reason: string, expires: string | null) => Promise<void>;
  onCancel: () => void;
}

export function SuppressDialog({ finding, onSubmit, onCancel }: SuppressDialogProps) {
  const [reason, setReason] = useState("");
  const [expires, setExpires] = useState("");
  const [missingReason, setMissingReason] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [underPath, setUnderPath] = useState(false);
  const [path, setPath] = useState(() => suggestedPath(finding));
  const [missingPath, setMissingPath] = useState(false);
  const [onlyRule, setOnlyRule] = useState(false);
  const [onlyScanner, setOnlyScanner] = useState(false);

  const selector = (): SecuritySelector => {
    if (!underPath) return { fingerprint: finding.fingerprint };
    return {
      path: path.trim(),
      ...(onlyRule ? { rule: finding.rule_id } : {}),
      ...(onlyScanner ? { scanner: finding.scanner_name } : {}),
    };
  };

  const submit = async () => {
    const noReason = reason.trim() === "";
    const noPath = underPath && path.trim() === "";
    setMissingReason(noReason);
    setMissingPath(noPath);
    if (noReason || noPath) return;
    setFailure(null);
    setSaving(true);
    try {
      await onSubmit(selector(), reason.trim(), expires === "" ? null : expires);
    } catch (e) {
      setFailure(SUPPRESS_TEXT.saveFailed(requestDetail(e)));
      setSaving(false);
    }
  };

  return (
    <Modal
      open
      onClose={onCancel}
      title={SUPPRESS_TEXT.title}
      footer={
        <>
          <Button variant="ghost" onClick={onCancel}>
            {SUPPRESS_TEXT.cancel}
          </Button>
          <Button variant="primary" onClick={() => void submit()} disabled={saving}>
            {SUPPRESS_TEXT.confirm}
          </Button>
        </>
      }
    >
      <p>{SUPPRESS_TEXT.lead(finding.rule_id, findingLocation(finding))}</p>
      <fieldset className={styles.scope}>
        <legend>{SUPPRESS_TEXT.scope}</legend>
        <label>
          <input type="radio" name="suppress-scope" checked={!underPath} onChange={() => setUnderPath(false)} />{" "}
          {SUPPRESS_TEXT.scopeFinding}
        </label>
        <label>
          <input type="radio" name="suppress-scope" checked={underPath} onChange={() => setUnderPath(true)} />{" "}
          {SUPPRESS_TEXT.scopePath}
        </label>
        {underPath && (
          <>
            <TextField
              label={SUPPRESS_TEXT.path}
              hint={SUPPRESS_TEXT.pathHint}
              error={missingPath ? SUPPRESS_TEXT.pathRequired : undefined}
              value={path}
              required
              className="mono"
              onChange={(e) => {
                setPath(e.target.value);
                if (e.target.value.trim() !== "") setMissingPath(false);
              }}
            />
            <label>
              <input type="checkbox" checked={onlyRule} onChange={(e) => setOnlyRule(e.target.checked)} />{" "}
              {SUPPRESS_TEXT.onlyRule(finding.rule_id)}
            </label>
            <label>
              <input type="checkbox" checked={onlyScanner} onChange={(e) => setOnlyScanner(e.target.checked)} />{" "}
              {SUPPRESS_TEXT.onlyScanner(finding.scanner_name)}
            </label>
          </>
        )}
      </fieldset>
      <TextareaField
        label={SUPPRESS_TEXT.reason}
        hint={SUPPRESS_TEXT.reasonHint}
        error={missingReason ? SUPPRESS_TEXT.reasonRequired : undefined}
        value={reason}
        required
        onChange={(e) => {
          setReason(e.target.value);
          if (e.target.value.trim() !== "") setMissingReason(false);
        }}
      />
      <TextField
        type="date"
        label={SUPPRESS_TEXT.expires}
        hint={SUPPRESS_TEXT.expiresHint}
        value={expires}
        onChange={(e) => setExpires(e.target.value)}
      />
      <p className="muted">{SUPPRESS_TEXT.where}</p>
      {failure !== null && (
        <p role="alert" className={styles.alert}>
          {failure}
        </p>
      )}
    </Modal>
  );
}
