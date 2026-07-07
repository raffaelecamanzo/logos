/*
 * Empty / Loading / Error states (S-193, FR-UI-23; frontend-design §1.1/§5/§9).
 * Honesty-as-design made reusable:
 *   - EmptyState — one muted sentence naming the producing CLI command in mono;
 *     never a fabricated figure or a bare spinner for missing data.
 *   - LoadingState — an accessible busy indicator (role=status, aria-live); its
 *     spin is disabled under prefers-reduced-motion (base.css), leaving a static
 *     mark + label.
 *   - ErrorPanel — the façade error text shown, never papered over (role=alert).
 */

import type { ReactNode } from "react";

import styles from "./States.module.css";

export interface EmptyStateProps {
  /** The muted explanation (e.g. "No coverage ingested — run"). */
  message: ReactNode;
  /** The producing CLI command, rendered in mono (e.g. "logos coverage ingest"). */
  command?: string;
}

export function EmptyState({ message, command }: EmptyStateProps) {
  return (
    <div className={styles.empty}>
      <p className={styles.emptyText}>
        {message}
        {command && (
          <>
            {" "}
            <code className={styles.command}>{command}</code>
          </>
        )}
      </p>
    </div>
  );
}

export interface LoadingStateProps {
  /** Accessible + visible label (e.g. "Loading health…"). */
  label?: string;
}

export function LoadingState({ label = "Loading…" }: LoadingStateProps) {
  return (
    <div className={styles.loading} role="status" aria-live="polite">
      <span className={styles.spinner} aria-hidden="true" />
      <span className={styles.loadingLabel}>{label}</span>
    </div>
  );
}

export interface ErrorPanelProps {
  children: ReactNode;
}

export function ErrorPanel({ children }: ErrorPanelProps) {
  return (
    <div className={styles.error} role="alert">
      <span className={styles.errorLabel}>Error</span>
      <div className={styles.errorBody}>{children}</div>
    </div>
  );
}
