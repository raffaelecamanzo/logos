/*
 * The shared, accessible component library (S-193, CR-050, FR-UI-23, ADR-44). The
 * single import surface every migrated view (S-186–S-191) consumes — so a view
 * renders exclusively through these tokens-driven, WCAG 2.1 AA components and a
 * theme is a token remap, never a per-view change.
 */

export { AppShell } from "./AppShell.tsx";
export type { AppShellProps } from "./AppShell.tsx";

export { Badge } from "./Badge.tsx";
export type { BadgeProps, BadgeTone } from "./Badge.tsx";

export { Button } from "./Button.tsx";
export type { ButtonProps, ButtonSize, ButtonVariant } from "./Button.tsx";

export { Callout } from "./Callout.tsx";
export type { CalloutProps, CalloutTone } from "./Callout.tsx";

export { Card } from "./Card.tsx";
export type { CardProps } from "./Card.tsx";

export { DataTable } from "./DataTable.tsx";
export type { Column, DataTableProps } from "./DataTable.tsx";

export { DEFAULT_TABLE_PAGE_SIZE } from "./table.constants.ts";

export { SelectField, TextField, TextareaField } from "./FormControls.tsx";
export type { SelectFieldProps, TextFieldProps, TextareaFieldProps } from "./FormControls.tsx";

export { Modal } from "./Modal.tsx";
export type { ModalProps } from "./Modal.tsx";

export { ScoreBar } from "./ScoreBar.tsx";
export type { ScoreBarProps, ScoreBarTone } from "./ScoreBar.tsx";

export { EmptyState, ErrorPanel, LoadingState } from "./States.tsx";
export type { EmptyStateProps, ErrorPanelProps, LoadingStateProps } from "./States.tsx";

export { Tabs } from "./Tabs.tsx";
export type { TabItem, TabsProps } from "./Tabs.tsx";

export { ThemeToggle } from "./ThemeToggle.tsx";

export { ToastProvider, useToast } from "./Toast.tsx";
export type { ToastOptions, ToastTone } from "./Toast.tsx";
