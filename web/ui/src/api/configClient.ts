/*
 * The Config editor's data-access layer (S-191, CR-049, FR-UI-12/13, FR-CF-06,
 * ADR-31) — the first **mutating** client surface in the SPA.
 *
 * Reads (`fetchConfig`) go through the GET read-model like every other view. The
 * three writes (Save / Apply / Save-secret) go through the foundation's
 * {@link apiMutate} seam, which echoes the per-session intent token in
 * `x-logos-intent` (NFR-SE-06) and fails loudly when no token was delivered to the
 * shell — so a token-less mutating request is never sent. The unchanged surface
 * accepts urlencoded form bodies on these three routes ([ADR-31]); the migration
 * only re-homes the client, it does not touch the endpoints or their guard.
 *
 * Member scope (S-250, FR-UI-29): in workspace mode every request here — reads AND
 * writes — carries the selected member (`?repo=`). The write side is the load-bearing
 * half: the editor reads the selected member's policy, so its Save/Apply must write
 * back to THAT member, never over the workspace default's file.
 *
 * Honesty-at-the-boundary: a non-2xx write throws a {@link ConfigMutateError}
 * carrying the server's status + verbatim detail text, so the view renders the
 * real validation fault (a `422` names the bad key/glob/range) rather than a
 * fabricated success ([NFR-RA-05]). The secret path never returns the response
 * body to the caller — only the masked outcome — so the key can never be echoed
 * onto a SPA surface ([NFR-SE-07]).
 */

import { apiFetch, apiUrl } from "./client.ts";
import { apiMutate } from "../intent.ts";
import { withMemberScope } from "../workspace/scope.ts";
import type {
  ConfigApplyOutcome,
  ConfigReadModel,
  ConfigWriteOutcome,
  SecretWriteOutcome,
  VerifyReport,
} from "./types.ts";

/** Which policy file a Save/Apply targets — the `file=` form value. */
export type PolicyFile = "config" | "rules";

/** A mutating config request the server rejected (non-2xx). Carries the status
 *  and the server's verbatim detail so the view surfaces the real fault. */
export class ConfigMutateError extends Error {
  constructor(
    public readonly status: number,
    public readonly detail: string,
  ) {
    super(`config write failed (HTTP ${status})`);
    this.name = "ConfigMutateError";
  }
}

const FORM_HEADERS = { "Content-Type": "application/x-www-form-urlencoded" };

/** Encode a flat record as an `application/x-www-form-urlencoded` body. */
function formBody(params: Record<string, string>): string {
  return Object.entries(params)
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`)
    .join("&");
}

/** Read the server's verbatim error text (trimmed) for an honest rejection. */
async function detailOf(res: Response): Promise<string> {
  try {
    return (await res.text()).trim();
  } catch {
    return "";
  }
}

/** `GET /api/v1/config` — both policy files + the masked chat key (FR-UI-12). A
 *  pure read; loading the editor mutates no store ([ADR-28]). */
export function fetchConfig(): Promise<ConfigReadModel> {
  return apiFetch<ConfigReadModel>("config");
}

/**
 * `POST /config/save` — validate-then-atomic-write of the candidate `content`
 * for `file` (FR-UI-12, BR-35). Resolves with the {@link ConfigWriteOutcome} on a
 * valid save; throws {@link ConfigMutateError} on an invalid candidate (`422`,
 * file left byte-identical — no partial write) or an I/O fault (`500`). Save runs
 * **no** pipeline; applying is the separate {@link applyConfig} step.
 */
export async function saveConfig(file: PolicyFile, content: string): Promise<ConfigWriteOutcome> {
  const res = await apiMutate(withMemberScope("/config/save"), {
    headers: FORM_HEADERS,
    body: formBody({ file, content }),
    credentials: "same-origin",
  });
  if (!res.ok) throw new ConfigMutateError(res.status, await detailOf(res));
  return (await res.json()) as ConfigWriteOutcome;
}

/**
 * `POST /config/apply` — the explicit Apply over the **saved** on-disk `file`
 * (FR-UI-13): a `config.toml` reconcile or a `rules.toml` gate re-eval. Posts only
 * `file=` (never `content`). Resolves with the internally-tagged
 * {@link ConfigApplyOutcome}; throws {@link ConfigMutateError} on failure.
 */
export async function applyConfig(file: PolicyFile): Promise<ConfigApplyOutcome> {
  const res = await apiMutate(withMemberScope("/config/apply"), {
    headers: FORM_HEADERS,
    body: formBody({ file }),
    credentials: "same-origin",
  });
  if (!res.ok) throw new ConfigMutateError(res.status, await detailOf(res));
  return (await res.json()) as ConfigApplyOutcome;
}

/**
 * `POST /config/secret` — write (or, with a blank key, clear) the chat API key
 * into the gitignored `secrets.toml` (FR-CF-06, NFR-SE-07). The key is write-only:
 * this resolves with the **masked** {@link SecretWriteOutcome} (presence + last-4)
 * and **never** returns the response body to the caller — a non-JSON 2xx resolves
 * to `null` ("saved, format not understood") rather than surface a body that could
 * in principle carry key material. Throws {@link ConfigMutateError} on a non-2xx.
 */
export async function saveSecret(apiKey: string): Promise<SecretWriteOutcome | null> {
  const res = await apiMutate(withMemberScope("/config/secret"), {
    headers: FORM_HEADERS,
    body: formBody({ api_key: apiKey }),
    credentials: "same-origin",
  });
  if (!res.ok) {
    // NFR-SE-07: never surface the secret route's response body — it could in
    // principle carry key material if the server ever echoed the request. Report
    // the status honestly with a fixed, body-free detail (the status label still
    // conveys the fault category) rather than the verbatim body the other routes
    // safely show.
    throw new ConfigMutateError(res.status, "the server rejected the key write");
  }
  const text = await res.text();
  try {
    return JSON.parse(text) as SecretWriteOutcome;
  } catch {
    // Never echo a non-JSON body on the secret path (NFR-SE-07) — report the
    // honest "saved but unexpected format" signal without the body.
    return null;
  }
}

/**
 * `POST /api/v1/verify` (S-207, CR-052, FR-UI-25, FR-GV-19) — the on-demand
 * **deep graph-consistency check**: reindexes the project into a throwaway
 * shadow store and diffs it against the live graph. Unlike the other config
 * mutations this posts no body (the handler reads none); it rides the same
 * intent-guarded {@link apiMutate} seam because it is the one mutating-method
 * slot the server's same-origin/intent guard admits ([NFR-SE-06], [ADR-31]) —
 * the endpoint itself performs no write to the live store. The shadow reindex
 * can run seconds-to-minutes, so callers must show an explicit in-flight state
 * ([FR-UI-07]) rather than a frozen control. Throws {@link ConfigMutateError}
 * on a non-2xx so a read/verify fault renders the honest error panel — never a
 * fabricated `CONSISTENT` ([NFR-RA-05]).
 */
export async function verifyGraph(): Promise<VerifyReport> {
  const res = await apiMutate(apiUrl("verify"), { credentials: "same-origin" });
  if (!res.ok) throw new ConfigMutateError(res.status, await detailOf(res));
  return (await res.json()) as VerifyReport;
}
