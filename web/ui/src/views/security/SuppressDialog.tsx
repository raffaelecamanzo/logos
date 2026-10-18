/*
 * SuppressDialog (S-672, FR-SC-12) — accept one finding's risk from the
 * Security page. A reason is required and an expiry optional; the dialog names
 * the committed file the suppression is written to (`.logos/security-ignore.toml`)
 * before anything is written. An empty reason is refused here, with no request
 * sent; a refusal from the server (a past expiry, a hand-spelled file) is shown
 * verbatim and the dialog stays open.
 */

import { useState } from "react";

import { requestDetail } from "../../api/securityClient.ts";
import type { SecurityFinding } from "../../api/types.ts";
import { Button, Modal, TextareaField, TextField } from "../../components/index.ts";
import { SUPPRESS_TEXT } from "../../copy/security.copy.ts";
import { findingLocation } from "./securityModel.ts";
import styles from "./SecurityView.module.css";

export interface SuppressDialogProps {
  /** The finding to suppress. */
  finding: SecurityFinding;
  /** Writes the suppression; rejects with the reason it was not written. */
  onSubmit: (reason: string, expires: string | null) => Promise<void>;
  onCancel: () => void;
}

export function SuppressDialog({ finding, onSubmit, onCancel }: SuppressDialogProps) {
  const [reason, setReason] = useState("");
  const [expires, setExpires] = useState("");
  const [missingReason, setMissingReason] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const submit = async () => {
    if (reason.trim() === "") {
      setMissingReason(true);
      return;
    }
    setFailure(null);
    setSaving(true);
    try {
      await onSubmit(reason.trim(), expires === "" ? null : expires);
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
