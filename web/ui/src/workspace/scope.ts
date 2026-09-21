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
 * Re-fetching on a switch is the *cache-key* concern, and it is React's:
 * `WorkspaceProvider` keys the mounted view subtree on the selected member, so a
 * switch remounts the views and every resource re-runs. The two must move together
 * — hence `setScopedMember` is called by the provider, never by a view.
 *
 * S-426 (FR-UI-35, NFR-RA-05) adds the **URL** half: `?repo=` is now also the
 * browser-URL vocabulary, so a workspace URL names the member it shows and can be
 * bookmarked or shared. Both halves here are pure — {@link memberFromSearch} takes a
 * query string and {@link urlWithMember} takes a URL, neither touches `window`. The
 * two callers that do are `WorkspaceContext` (which reads `window.location.search`)
 * and `router.tsx` (which owns every `window.history` write).
 *
 * All three directions — the URL read, the URL write, and the transport scope —
 * normalise through the ONE rule in {@link normaliseMember}, which is the same rule
 * the server applies in `web/src/member.rs` (`requested_member` → `api_v1::opt_param`).
 * The two spellings cannot share code across the process boundary, so they share a
 * case table instead: `repo-param-cases.txt`, asserted by `scope.test.ts` on this
 * side and by `member.rs`'s own tests on the other.
 */

/** The query param the member scope rides on — in a request URL and, since S-426,
 *  in the browser URL too. Named once here and used by every site that reads or
 *  writes it, so the wire spelling has one definition. */
export const REPO_PARAM = "repo";

/** The member every `/api/v1` read is scoped to, or `null` for single-root/unscoped. */
let scoped: string | null = null;

/**
 * The one `?repo=` rule: trim, and read an empty result as **unscoped** — never a
 * member named `""`.
 *
 * This mirrors the server's `api_v1::opt_param`, which `member.rs`'s
 * `requested_member` applies to the same param on the same request. The two are
 * pinned against one shared case table (`repo-param-cases.txt`) so a drift in
 * either reds a test rather than silently making `?repo=%20api` mean two different
 * members on the two sides of the loopback.
 *
 * It trims the Unicode **White_Space** set explicitly rather than calling
 * `String.trim()`, and that is not a stylistic choice. JavaScript's `trim` strips
 * one character the property does not contain — U+FEFF, the BOM — while Rust's
 * `str::trim` is the property exactly. With `trim()` here, `?repo=%EF%BB%BFapi`
 * meant the member `api` to this client and a member the workspace does not have to
 * the server: the SPA would scope every read to `api` and the server would `404`
 * each one. `\p{White_Space}` is the same property on both sides, so the sets are
 * now identical by construction; the BOM case is a row in the shared table.
 */
export function normaliseMember(raw: string | null | undefined): string | null {
  const trimmed = raw?.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, "");
  return trimmed ? trimmed : null;
}

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
  scoped = normaliseMember(member);
}

/**
 * The member a query string names, or `null` when it names none — the URL half of
 * {@link normaliseMember}.
 *
 * `search` is `window.location.search` (leading `?` optional). Decoding is
 * `URLSearchParams`', which is form-urlencoded: `%20` and `+` are both a space, so
 * `?repo=+` is blank and therefore unscoped. A repeated `repo` takes the **last**
 * value, which is what the server's map-shaped decode of the same query yields.
 */
export function memberFromSearch(search: string): string | null {
  const params = new URLSearchParams(search.startsWith("?") ? search.slice(1) : search);
  return normaliseMember(params.getAll(REPO_PARAM).at(-1));
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
  return `${path}${sep}${REPO_PARAM}=${encodeURIComponent(scoped)}`;
}

/** The decoded key of one raw `k=v` pair, or the raw key if it will not decode.
 *
 *  Percent escapes only. Form-urlencoding's `+`-for-space is deliberately NOT undone:
 *  the one key this is ever compared against is `repo`, which contains no space, so
 *  no `+`-bearing spelling could ever decode to it — undoing it would be a
 *  transformation that cannot change any answer. */
function pairKey(pair: string): string {
  const raw = pair.split("=", 1)[0];
  try {
    return decodeURIComponent(raw);
  } catch {
    // A malformed escape is not a `repo` we failed to recognise; it is a key we
    // cannot read, and it must be carried through rather than take navigation down.
    return raw;
  }
}

/**
 * `url` with its `?repo=` set to `member` — the browser-URL write (S-426).
 *
 * `member === null` returns `url` **verbatim**, and that is the single-root
 * guarantee rather than an optimisation ([ADR-52]): single-root never has a scope,
 * so no navigation can write a `?repo=`, and a hand-typed one is left exactly where
 * the user typed it — inert, like every other unrecognised query param has always
 * been. It is neither honoured nor stripped, because stripping it would be this
 * mode noticing a param it is supposed to be blind to.
 *
 * Every other pair is carried across **byte-for-byte**: the query is edited pair by
 * pair rather than round-tripped through `URLSearchParams`, whose re-serialisation
 * would rewrite an existing `%20` as `+` in somebody else's value.
 */
export function urlWithMember(url: string, member: string | null): string {
  if (member === null) return url;
  const hashAt = url.indexOf("#");
  const hash = hashAt === -1 ? "" : url.slice(hashAt);
  const bare = hashAt === -1 ? url : url.slice(0, hashAt);
  const queryAt = bare.indexOf("?");
  const path = queryAt === -1 ? bare : bare.slice(0, queryAt);
  const query = queryAt === -1 ? "" : bare.slice(queryAt + 1);
  const kept = query
    .split("&")
    .filter((pair) => pair !== "" && pairKey(pair) !== REPO_PARAM);
  kept.push(`${REPO_PARAM}=${encodeURIComponent(member)}`);
  return `${path}?${kept.join("&")}${hash}`;
}
