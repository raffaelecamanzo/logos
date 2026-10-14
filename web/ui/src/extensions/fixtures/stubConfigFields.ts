/*
 * The stub extension's config field groups (S-730, FR-UI-54): one field of every
 * kind over the stub's own `[stub]` table — the table its manifest config panel
 * names (`stubInfo()` in `../../test/extensions.tsx`) — so the Config forms and the
 * registry are tested without naming any real extension's tables or keys.
 */

import { STUB_UI } from "../../test/extensions.tsx";
import type { ConfigFieldGroup, ExtensionUi } from "../registry.ts";

/** The read-only note the stub's member group carries. */
export const STUB_NOTE = "Stub settings are read by the stub extension only.";

/** One field of each kind, in the stub's table. */
export const STUB_FIELDS: ConfigFieldGroup["fields"] = [
  { table: "stub", key: "count", kind: "int", label: "Stub count", help: "How many stubs.", placeholder: "e.g. 3", defaultHint: "Default: 1" },
  { table: "stub", key: "ratio", kind: "float", label: "Stub ratio", help: "A share between 0 and 1.", placeholder: "0.5", defaultHint: "Default: 0.25" },
  { table: "stub", key: "enabled", kind: "bool", label: "Stub enabled", help: "Whether the stub runs.", defaultHint: "Default: true" },
  { table: "stub", key: "names", kind: "list", label: "Stub names", help: "One per line.", placeholder: "alpha", defaultHint: "Default: (empty)" },
  { table: "stub", key: "title", kind: "str", label: "Stub title", help: "Shown in the stub view.", placeholder: "Untitled", defaultHint: "Default: unset" },
  { table: "stub", key: "mode", kind: "choice", options: ["fast", "slow"], label: "Stub mode", help: "How the stub runs.", defaultHint: "Default: fast" },
];

/** The stub's member group: every kind, under a legend, with a read-only note. */
export const STUB_MEMBER_GROUPS: readonly ConfigFieldGroup[] = [
  { legend: "[stub]", note: STUB_NOTE, fields: STUB_FIELDS },
];

/** The stub's workspace group: a subset of the descriptors under its own legend, no
 *  note — distinct from the member group, so a view wired to the wrong scope shows. */
export const STUB_WORKSPACE_GROUPS: readonly ConfigFieldGroup[] = [
  { legend: "[stub] (workspace tier)", fields: [STUB_FIELDS[0], STUB_FIELDS[5]] },
];

/** The stub's code with its field groups registered against its config panel. */
export const STUB_FIELDS_UI: ExtensionUi = {
  ...STUB_UI,
  configFields: { "stub-config": { member: STUB_MEMBER_GROUPS, workspace: STUB_WORKSPACE_GROUPS } },
};
