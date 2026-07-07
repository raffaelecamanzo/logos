/*
 * Button (S-193, FR-UI-23). An accessible, token-driven button. Variants:
 *   - primary  — the one emphatic action; solid signal-red, white ink (≥4.5:1).
 *   - secondary— bordered neutral surface (the default).
 *   - ghost    — text-only, hover tint; for low-emphasis / toolbar actions.
 * Keyboard-operable as a native <button>; the global :focus-visible ring applies.
 */

import type { ButtonHTMLAttributes, ReactNode } from "react";

import styles from "./Button.module.css";

export type ButtonVariant = "primary" | "secondary" | "ghost";
export type ButtonSize = "sm" | "md";

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  children: ReactNode;
}

export function Button({
  variant = "secondary",
  size = "md",
  className,
  type,
  children,
  ...rest
}: ButtonProps) {
  const cls = [styles.button, styles[variant], styles[size], className]
    .filter(Boolean)
    .join(" ");
  return (
    // A bare <button> defaults to type="submit" inside a form — default to "button"
    // so a library button never submits a form by accident.
    <button type={type ?? "button"} className={cls} {...rest}>
      {children}
    </button>
  );
}
