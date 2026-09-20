/*
 * The workspace-probe fault badge (S-250 behaviour, extracted from
 * `MemberSelector` in S-425).
 *
 * The roster probe hitting a genuine fault (a 500, a transport failure) is NOT a
 * plain repository. There is no roster to render, so the shell falls back to the
 * unscoped single-root layout — and something has to say so, or a broken read
 * masquerades as "this is not a workspace" and leaves the user reading one member
 * as if it were the whole application (NFR-RA-05, NFR-CC-04).
 *
 * It is separate from `MemberSelector` because of WHERE each one belongs. S-425
 * moved the selector into the sidebar's Service section, and that section exists
 * only in workspace mode — which a fault never reaches (`WorkspaceContext` settles
 * the mode to `single` and records the error). A fault reported from inside the
 * selector would therefore be a fault reported nowhere. The header renders in every
 * mode, so the badge stays there, where it has always been rendered and at the
 * width concession S-317 measured for it.
 *
 * It is a different signal about a different subject from the header's graph-state
 * readout — the MEMBER AXIS, not the graph — so it deliberately does not give way
 * at the narrow rung the readout does; see `Header.test.tsx`.
 */

import { Badge } from "../components/index.ts";
import { useWorkspace } from "../workspace/WorkspaceContext.tsx";
// Its one rule stays in `MemberSelector.module.css` rather than moving to a module
// of this component's own. A CSS-module class name is derived from the stylesheet's
// PATH, so relocating the rule renames `.fault` in the served single-root header —
// measured, not assumed: `_fault_fab455` became `_fault_41911d`. A degraded
// workspace settles the mode to `single`, so that header IS a single-root header,
// and ADR-52's byte-for-byte guarantee is the binding constraint on this story. The
// rule is documented in that file, next to the row it concedes width to.
import styles from "./MemberSelector.module.css";

export function WorkspaceFault() {
  const { error } = useWorkspace();
  if (!error) return null;
  return (
    <Badge tone="red" className={styles.fault}>
      Workspace status unavailable
    </Badge>
  );
}
