/*
 * The pieces a per-member security table shares (S-674, S-790, FR-SC-16,
 * FR-UI-36): where a member's Security page lives, the Member column that links
 * to it, and what a figure cell says when the member has nothing to count.
 *
 * Workspace Security and the Workspace Dashboard's Security widget both state a
 * row per member, so a member is named, linked and absent in one spelling —
 * never "never run" on one page and a count of zero on the other (BR-66). Kept
 * out of both views so neither imports the other. Workspace Coverage (S-740)
 * reuses the Member column with its own page's href, so a member's name and its
 * unread reason are spelled once across the three tables.
 */

import type { Column } from "../../components/index.ts";
import { WORKSPACE_SECURITY_TEXT } from "../../copy/workspaceSecurity.copy.ts";
import { urlWithMember } from "../../workspace/scope.ts";

/** One member's row of a federated read (the shape of `MemberResult` in
 *  `api/types.ts`). Typed here, structurally, so this module adds no importer to
 *  that file's fan-in, which sits at the architecture rule's limit. */
interface MemberRow<T> {
  member: string;
  result?: T;
  error?: string;
}

/** The member-scoped Security page (S-670) for `member`: its route with the
 *  member in `?repo=`, the one way a URL names a member (S-426). */
export function memberSecurityHref(member: string): string {
  return urlWithMember("/security", member);
}

/** What a figure cell says when the member has nothing to count from: it was
 *  never scanned (nothing looked, which is not a clean zero, BR-66), or it could
 *  not be read at all. */
export function absentFigure(row: MemberRow<unknown>): string {
  return row.result ? WORKSPACE_SECURITY_TEXT.neverRun : WORKSPACE_SECURITY_TEXT.notRead;
}

/** The Member column: the member's name linked to its own page — its Security
 *  page unless `href` names another — and the reason beneath it when the member
 *  could not be opened or read (NFR-CC-04). */
export function memberColumn<T>(href: (member: string) => string = memberSecurityHref): Column<MemberRow<T>> {
  return {
    key: "member",
    header: "Member",
    mono: true,
    cell: (row) => (
      <>
        <a href={href(row.member)}>{row.member}</a>
        {row.error && (
          <>
            <br />
            <span className="muted">{row.error}</span>
          </>
        )}
      </>
    ),
    sortValue: (row) => row.member,
  };
}
