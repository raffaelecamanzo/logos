/*
 * The unknown-member refusal (S-426, FR-UI-35, NFR-RA-05) — what the shell renders
 * in place of every view when the URL's `?repo=` names a member this workspace does
 * not have.
 *
 * It exists because the honest alternative is unavailable and the convenient one is
 * a lie. `?repo=ghost` cannot be answered: there is no `ghost` to answer for. The
 * convenient thing is to fall back to the default member — and that produces a page
 * that is `200`-shaped, fully populated, and wrong: one member's figures under
 * another member's name, with nothing on screen admitting the substitution. That is
 * the failure NFR-RA-05 names, and it is exactly what the server already refuses
 * with a `404` (`web/src/member.rs`'s `unknown_member`). The client must not
 * re-merge what the server split.
 *
 * So this renders INSTEAD of the view, never beside it — `App.tsx` mounts no view
 * at all while it is up, and the transport scope is `null`, so no member's figures
 * reach the screen.
 *
 * It names the members the workspace DOES have, as buttons. The roster is already in
 * hand (the shell probed it at boot), so naming them costs no request and starts no
 * engine — and a dead end that lists the way out is the difference between a refusal
 * and a wall. Selecting one is the ordinary member switch: it re-scopes the
 * transport and rewrites the URL, so the refusal is also self-clearing.
 *
 * Not to be confused with a member that IS in the roster but whose engine will not
 * start. That is a `500` from the read itself, surfaced by the view that issued it,
 * and the two claims are deliberately different: "no such member" sends the user
 * hunting for a typo, "could not be started" sends them to the store.
 */

import { Button, ErrorPanel } from "../components/index.ts";
import styles from "./UnknownMember.module.css";
import { useWorkspace } from "./WorkspaceContext.tsx";

export function UnknownMember() {
  const { unknownMember, workspace, members, selectMember } = useWorkspace();
  if (unknownMember === null) return null;

  return (
    <ErrorPanel>
      <p className={styles.claim}>
        No workspace member <code className={styles.name}>{unknownMember}</code>
        {workspace ? (
          <>
            {" "}
            in <code className={styles.name}>{workspace}</code>
          </>
        ) : null}
        .
      </p>
      {members.length > 0 ? (
        <>
          {/* The roster, named rather than merely counted: the user asked for a
              member by name, so the answer is the names that exist. */}
          <p className={styles.rosterLabel}>This workspace has:</p>
          <ul className={styles.roster}>
            {members.map((name) => (
              <li key={name}>
                <Button size="sm" onClick={() => selectMember(name)}>
                  {name}
                </Button>
              </li>
            ))}
          </ul>
        </>
      ) : null}
    </ErrorPanel>
  );
}
