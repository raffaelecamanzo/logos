/*
 * Toast (S-193, FR-UI-23). Transient notifications surfaced through a context
 * provider + `useToast()` hook. The region is an `aria-live` landmark so screen
 * readers announce new toasts; error toasts use `role="alert"` (assertive), the
 * rest `role="status"` (polite). Each toast auto-dismisses and is manually
 * dismissable. Honesty-as-design: an error toast shows the real message, never a
 * fabricated success.
 */

import { createContext, useCallback, useContext, useRef, useState } from "react";
import type { ReactNode } from "react";

import styles from "./Toast.module.css";

export type ToastTone = "info" | "success" | "warn" | "error";

export interface ToastOptions {
  tone?: ToastTone;
  message: ReactNode;
  /** Auto-dismiss delay in ms (default 5000). */
  durationMs?: number;
}

interface ToastInstance extends Required<Omit<ToastOptions, "durationMs">> {
  id: number;
}

interface ToastContextValue {
  toast: (opts: ToastOptions) => void;
}

const ToastContext = createContext<ToastContextValue | null>(null);

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<ToastInstance[]>([]);
  const nextId = useRef(0);

  const dismiss = useCallback((id: number) => {
    setToasts((list) => list.filter((t) => t.id !== id));
  }, []);

  const toast = useCallback(
    ({ tone = "info", message, durationMs = 5000 }: ToastOptions) => {
      const id = nextId.current++;
      setToasts((list) => [...list, { id, tone, message }]);
      if (durationMs > 0) {
        window.setTimeout(() => dismiss(id), durationMs);
      }
    },
    [dismiss],
  );

  return (
    <ToastContext.Provider value={{ toast }}>
      {children}
      {/* Polite region for status; alert toasts carry their own assertive role. */}
      <div className={styles.region} role="region" aria-label="Notifications">
        {toasts.map((t) => (
          <div
            key={t.id}
            role={t.tone === "error" ? "alert" : "status"}
            className={`${styles.toast} ${styles[t.tone]}`}
          >
            <span className={styles.message}>{t.message}</span>
            <button
              type="button"
              className={styles.dismiss}
              aria-label="Dismiss notification"
              onClick={() => dismiss(t.id)}
            >
              <span aria-hidden="true">✕</span>
            </button>
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

/** Access the toast dispatcher. Throws if used outside a ToastProvider. */
export function useToast(): ToastContextValue {
  const ctx = useContext(ToastContext);
  if (!ctx) throw new Error("useToast must be used within a ToastProvider");
  return ctx;
}
