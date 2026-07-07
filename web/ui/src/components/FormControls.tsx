/*
 * Form controls (S-193, FR-UI-23). Accessible inputs with a programmatic
 * label↔control association, an optional hint and error wired via
 * `aria-describedby`, and `aria-invalid` on error. Text, select, and textarea
 * variants share one labelled shell. The global :focus-visible ring applies; an
 * error adds a red edge (colour is paired with the error text, never alone).
 */

import { useId } from "react";
import type {
  InputHTMLAttributes,
  ReactNode,
  SelectHTMLAttributes,
  TextareaHTMLAttributes,
} from "react";

import styles from "./FormControls.module.css";

interface FieldShellProps {
  id: string;
  label: ReactNode;
  hint?: ReactNode;
  error?: ReactNode;
  hintId?: string;
  errorId?: string;
  children: ReactNode;
}

function FieldShell({ id, label, hint, error, hintId, errorId, children }: FieldShellProps) {
  return (
    <div className={styles.field}>
      <label className={styles.label} htmlFor={id}>
        {label}
      </label>
      {children}
      {hint && !error && (
        <p id={hintId} className={styles.hint}>
          {hint}
        </p>
      )}
      {error && (
        <p id={errorId} className={styles.error} role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

function describedBy(hint?: ReactNode, error?: ReactNode, hintId?: string, errorId?: string) {
  const ids: string[] = [];
  if (error) ids.push(errorId!);
  else if (hint) ids.push(hintId!);
  return ids.length ? ids.join(" ") : undefined;
}

type BaseFieldProps = { label: ReactNode; hint?: ReactNode; error?: ReactNode };

export type TextFieldProps = BaseFieldProps &
  Omit<InputHTMLAttributes<HTMLInputElement>, "id">;

export function TextField({ label, hint, error, className, ...rest }: TextFieldProps) {
  const id = useId();
  const hintId = `${id}-hint`;
  const errorId = `${id}-error`;
  return (
    <FieldShell id={id} label={label} hint={hint} error={error} hintId={hintId} errorId={errorId}>
      <input
        id={id}
        className={[styles.input, error ? styles.invalid : "", className].filter(Boolean).join(" ")}
        aria-invalid={error ? true : undefined}
        aria-describedby={describedBy(hint, error, hintId, errorId)}
        {...rest}
      />
    </FieldShell>
  );
}

export type SelectFieldProps = BaseFieldProps &
  Omit<SelectHTMLAttributes<HTMLSelectElement>, "id"> & { children: ReactNode };

export function SelectField({ label, hint, error, className, children, ...rest }: SelectFieldProps) {
  const id = useId();
  const hintId = `${id}-hint`;
  const errorId = `${id}-error`;
  return (
    <FieldShell id={id} label={label} hint={hint} error={error} hintId={hintId} errorId={errorId}>
      <select
        id={id}
        className={[styles.input, styles.select, error ? styles.invalid : "", className]
          .filter(Boolean)
          .join(" ")}
        aria-invalid={error ? true : undefined}
        aria-describedby={describedBy(hint, error, hintId, errorId)}
        {...rest}
      >
        {children}
      </select>
    </FieldShell>
  );
}

export type TextareaFieldProps = BaseFieldProps &
  Omit<TextareaHTMLAttributes<HTMLTextAreaElement>, "id">;

export function TextareaField({ label, hint, error, className, ...rest }: TextareaFieldProps) {
  const id = useId();
  const hintId = `${id}-hint`;
  const errorId = `${id}-error`;
  return (
    <FieldShell id={id} label={label} hint={hint} error={error} hintId={hintId} errorId={errorId}>
      <textarea
        id={id}
        className={[styles.input, styles.textarea, error ? styles.invalid : "", className]
          .filter(Boolean)
          .join(" ")}
        aria-invalid={error ? true : undefined}
        aria-describedby={describedBy(hint, error, hintId, errorId)}
        {...rest}
      />
    </FieldShell>
  );
}
