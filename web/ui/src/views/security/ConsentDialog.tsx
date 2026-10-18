/*
 * ConsentDialog (S-673, FR-SC-07, FR-SC-15) — the first-use dialog before a scan
 * from the Security page. It names every scanner that may reach the network and
 * says where the answer is kept: once per project, in the user-level config,
 * never in the repository. Accepting starts the scan with consent, and the
 * server records it; declining starts nothing.
 */

import { Button, Modal } from "../../components/index.ts";
import { CONSENT_TEXT } from "../../copy/security.copy.ts";
import styles from "./SecurityView.module.css";

export interface ConsentDialogProps {
  /** The scanners the project's consent does not name yet; the dialog is open
   *  while this is non-empty. */
  scanners: readonly string[];
  onAccept: () => void;
  onDecline: () => void;
}

export function ConsentDialog({ scanners, onAccept, onDecline }: ConsentDialogProps) {
  return (
    <Modal
      open={scanners.length > 0}
      onClose={onDecline}
      title={CONSENT_TEXT.title}
      footer={
        <>
          <Button variant="ghost" onClick={onDecline}>
            {CONSENT_TEXT.decline}
          </Button>
          <Button variant="primary" onClick={onAccept}>
            {CONSENT_TEXT.accept}
          </Button>
        </>
      }
    >
      <p>{CONSENT_TEXT.lead}</p>
      <ul className={styles.list} aria-label="Scanners that may reach the network">
        {scanners.map((s) => (
          <li key={s} className="mono">
            {s}
          </li>
        ))}
      </ul>
      <p className="muted">{CONSENT_TEXT.where}</p>
    </Modal>
  );
}
