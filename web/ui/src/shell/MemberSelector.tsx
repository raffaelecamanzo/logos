/*
 * The member/service selector (S-250, CR-061, FR-UI-29) — the shell's project axis,
 * and the SPA's only net-new shell primitive.
 *
 * Rendered in **workspace mode only**, on the sidebar's Service-section header row
 * and nowhere else (S-425, FR-UI-35, ADR-66 §4, frontend-design §3): the control
 * sits inside the boundary it governs, rather than in the app header above every
 * section including the one it does not reach. In single-root mode this component
 * returns `null` — and the section that would have held it is not rendered either,
 * so the sidebar is byte-for-byte the one it has always been (there is no member
 * axis in a plain repo, so offering one would be a lie).
 *
 * It carries no label element of its own. The section heading beside it IS its
 * label, named through `labelledBy`: the row is `SERVICE [ orders ▾ ]`, and a
 * second word for the same thing would not fit the 232px column and would read as
 * two controls. That heading is text at every breakpoint (`Sidebar.module.css`),
 * so the accessible name can never be the thing a narrow viewport drops.
 *
 * Selecting a member re-scopes the transport, re-keys every member-scoped view (see
 * `WorkspaceContext` and `App.tsx`), and — since S-426 — writes the member into the
 * URL, so a switch re-fetches rather than showing one member's figures under another
 * member's name, and the URL it leaves behind names what is on screen. The URL write
 * replaces the current history entry rather than pushing one: a switch on the view
 * you are already looking at must not cost a press of Back.
 *
 * It lists names and nothing else. The shell's roster probe is deliberately
 * engine-free (NFR-PE-10), so it does NOT know which members are indexed — and a
 * selector that guessed at that would be fabricating (NFR-CC-04). Per-member index
 * state is shown where it is actually read: the Workspace tab's service map.
 *
 * A FAILED probe is not this component's to report. It renders in a section that
 * exists only in workspace mode, and a probe fault falls back to the single-root
 * layout — so a fault reported here would be a fault reported nowhere. It is stated
 * by `WorkspaceFault`, which the header renders in every mode (NFR-RA-05).
 */

import { useWorkspace } from "../workspace/WorkspaceContext.tsx";
import styles from "./MemberSelector.module.css";

/** `labelledBy` is the id of the section heading that names this control. It is a
 *  required prop rather than a default: the control has no label of its own, so a
 *  caller that forgets one ships a `<select>` with no accessible name. */
export function MemberSelector({ labelledBy }: { labelledBy: string }) {
  const { mode, members, member, selectMember } = useWorkspace();

  // Single-root (or still probing): no member axis exists — render nothing.
  if (mode !== "workspace" || members.length === 0) return null;

  return (
    <select
      // Nothing in-tree reads this id — the label that used it is gone and the
      // tests query by role. It is kept as the control's stable handle for manual
      // testing and out-of-tree automation; naming it is cheaper than a rename
      // later.
      id="workspace-member"
      className={styles.select}
      aria-labelledby={labelledBy}
      value={member ?? ""}
      onChange={(e) => selectMember(e.target.value)}
    >
      {members.map((name) => (
        <option key={name} value={name}>
          {name}
        </option>
      ))}
    </select>
  );
}
