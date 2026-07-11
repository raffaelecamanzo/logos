/*
 * The active **member scope** (S-250, CR-061, FR-UI-29) — the transport half of
 * the workspace member selector.
 *
 * In workspace mode the shell's selector picks one member, and every existing
 * view must read that member's figures. Rather than thread a member through every
 * view's fetch call, the selection lives here as one module-level value that the
 * `/api/v1` URL builder (`api/client.ts`) reads and appends as `?repo=<member>` —
 * the same seam `src/intent.ts` uses for the per-session intent token. The server
 * resolves that param to the member's engine (`web/src/member.rs`).
 *
 * In single-root mode the scope is `null` and **no param is ever appended**, so
 * every request is byte-for-byte the one a pre-workspace SPA sent ([ADR-52]).
 *
 * This is the transport concern only. Re-fetching on a switch is the *cache-key*
 * concern, and it is React's: `WorkspaceProvider` keys the mounted view subtree on
 * the selected member, so a switch remounts the views and every resource re-runs.
 * The two must move together — hence `setScopedMember` is called by the provider,
 * never by a view.
 */

/** The member every `/api/v1` read is scoped to, or `null` for single-root/unscoped. */
let scoped: string | null = null;

/** The member `/api/v1` reads are currently scoped to (`null` ⇒ no `?repo=`). */
export function scopedMember(): string | null {
  return scoped;
}

/**
 * Scope every subsequent `/api/v1` read to `member` (or clear the scope with
 * `null`). Called by {@link WorkspaceProvider} as the selection changes; a blank
 * name is normalised to "unscoped" so a `?repo=` is never sent empty.
 */
export function setScopedMember(member: string | null): void {
  const trimmed = member?.trim();
  scoped = trimmed ? trimmed : null;
}

/**
 * Append the active member scope to an absolute path — the **mutating** seam's
 * counterpart to `apiUrl`'s injection on reads (`api/configClient.ts`).
 *
 * This one matters for honesty, not symmetry: the Config tab *reads* the selected
 * member's policy, so its Save/Apply must *write* that same member. Without the
 * scope here the editor would show member X's config and save it over the default
 * member's — the exact class of silent cross-member write the selector must never
 * enable. The server resolves the param identically on GET and POST
 * (`member::MemberEngine`), and the guards match on `uri().path()`, so a query
 * param cannot affect the admitted-route or intent checks.
 *
 * Unscoped (single-root, or workspace-unscoped) it returns `path` verbatim.
 */
export function withMemberScope(path: string): string {
  if (!scoped) return path;
  const sep = path.includes("?") ? "&" : "?";
  return `${path}${sep}repo=${encodeURIComponent(scoped)}`;
}
