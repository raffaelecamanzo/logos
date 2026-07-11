/*
 * The member/service selector (S-250, CR-061, FR-UI-29) — the shell's project axis,
 * and the SPA's only net-new shell primitive.
 *
 * Rendered in **workspace mode only**: in single-root mode this component returns
 * `null` and the header is byte-for-byte the one it has always been (there is no
 * member axis in a plain repo, so offering one would be a lie).
 *
 * Selecting a member re-scopes the transport and re-keys every view (see
 * `WorkspaceContext`), so a switch re-fetches rather than showing one member's
 * figures under another member's name. A member with no index yet is labelled
 * "awaiting index" in the option itself — visible and selectable, but never
 * presented as if it had data (NFR-CC-04).
 */

import { Badge } from "../components/index.ts";
import { useWorkspace } from "../workspace/WorkspaceContext.tsx";
import styles from "./MemberSelector.module.css";

export function MemberSelector() {
  const { mode, workspace, members, member, selectMember, error } = useWorkspace();

  // The probe hit a genuine fault (not a 404 "this is not a workspace"). Say so:
  // a broken read must never masquerade as a plain repo, which would silently hide
  // the workspace axis and leave the user reading one member as if it were the
  // whole app (NFR-RA-05, NFR-CC-04).
  if (error) return <Badge tone="red">Workspace status unavailable</Badge>;

  // Single-root (or still probing): no member axis exists — render nothing.
  if (mode !== "workspace" || members.length === 0) return null;

  return (
    <div className={styles.selector}>
      <label className={styles.label} htmlFor="workspace-member">
        {workspace ? `${workspace} ·` : ""} Service
      </label>
      <select
        id="workspace-member"
        className={styles.select}
        value={member ?? ""}
        onChange={(e) => selectMember(e.target.value)}
      >
        {members.map((m) => (
          <option key={m.name} value={m.name}>
            {m.indexed ? m.name : `${m.name} (awaiting index)`}
          </option>
        ))}
      </select>
    </div>
  );
}
