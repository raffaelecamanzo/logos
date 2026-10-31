/*
 * The Security page's scan-job and triage client (S-673, FR-SC-15, FR-SC-07;
 * S-672, FR-SC-12; NFR-SE-06).
 *
 * The read (`fetchScanJob`) is a GET like every other view's. Start, cancel,
 * suppress, unsuppress and reconfirm go
 * through the {@link apiMutate} seam, which echoes the per-session intent token
 * the server's same-origin/intent guard requires — the same path the Config
 * editor's writes take (`configClient.ts`). Every request carries the selected
 * member (`?repo=`) in workspace mode, so the scan runs, and is cancelled, on the
 * member the page shows.
 *
 * A start without consent while scanners lack it is not an error: the server
 * answers `409` with the scanners, and the caller shows the consent dialog.
 *
 * The scanner install (S-747, FR-SC-24) follows the same shape: `fetchInstallJob`
 * reads the member's `tool-install` job, `startInstall` / `cancelInstall` go
 * through {@link apiMutate}, and a start with a line still unanswered comes back
 * as the plan the consent dialog shows. A `409` without consent data — an install
 * or a scan already running — is a refusal like any other.
 */

import { apiMutate } from "../intent.ts";
import { withMemberScope } from "../workspace/scope.ts";
import { apiFetch } from "./client.ts";
import { apiErrorDetail } from "./configClient.ts";
import type {
  InstallConsentRequired,
  InstallJob,
  InstallJobModel,
  InstallJobStarted,
  ScanConsentRequired,
  ScanJob,
  ScanJobModel,
  ScanJobStarted,
  SecurityIgnoreWrite,
  SecurityInstallAsk,
  SecurityInstallPlan,
  SecuritySelector,
} from "./types.ts";

const SCAN_PATH = "/api/v1/security/scan";
const INSTALL_PATH = "/api/v1/security/install";

/** A start, cancel, suppress or unsuppress the server rejected. Carries the
 *  status and the server's verbatim detail, so the page states the real fault. */
export class SecurityRequestError extends Error {
  constructor(
    public readonly status: number,
    public readonly detail: string,
  ) {
    super(`the security request failed (HTTP ${status})`);
    this.name = "SecurityRequestError";
  }
}

/** What a failed security request says: the server's detail for a refusal,
 *  else the error's own message (a network fault). */
export function requestDetail(e: unknown): string {
  if (e instanceof SecurityRequestError) return e.detail || `HTTP ${e.status}`;
  return e instanceof Error ? e.message : String(e);
}

/** What a start resolved to: a running job (new or joined), or the scanners
 *  whose consent is still missing. */
export type ScanStart =
  | { kind: "started"; started: ScanJobStarted }
  | { kind: "consent"; scanners: string[] };

/** `GET /api/v1/security/scan` — the member's scan, running or last run; `null`
 *  when none ran in this serve. */
export async function fetchScanJob(): Promise<ScanJob | null> {
  const model = await apiFetch<ScanJobModel>("security/scan");
  return model.job ?? null;
}

/** `POST /api/v1/security/scan` — start the scan, or join the one running.
 *  `consent` names the scanners the consent dialog showed and the user
 *  accepted: the server records consent outside the repository for these
 *  alone, and answers `409` when a scanner it did not show still needs it. */
export async function startScan(consent: readonly string[]): Promise<ScanStart> {
  const res = await apiMutate(withMemberScope(SCAN_PATH), {
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ consent }),
    credentials: "same-origin",
  });
  if (res.status === 409) {
    const body = (await res.json()) as Partial<ScanConsentRequired>;
    if (Array.isArray(body.consent_required)) return { kind: "consent", scanners: body.consent_required };
    // An install running for the member: refused, not a question.
    throw new SecurityRequestError(res.status, body.error ?? "");
  }
  if (!res.ok) throw new SecurityRequestError(res.status, await apiErrorDetail(res));
  return { kind: "started", started: (await res.json()) as ScanJobStarted };
}

/** `POST /api/v1/security/scan/cancel` — cancel the running scan: every scanner
 *  process is killed and none starts after. */
export async function cancelScan(): Promise<ScanJob | null> {
  return cancelJob<ScanJob>(SCAN_PATH);
}

/** What an install start resolved to: a running job (new or joined), or the
 *  plan the consent dialog shows while a line is unanswered. */
export type InstallStart = { kind: "started"; started: InstallJobStarted } | { kind: "consent"; plan: SecurityInstallPlan };

/** `GET /api/v1/security/install` — the member's install, running or last run;
 *  `null` when none ran in this serve. */
export async function fetchInstallJob(): Promise<InstallJob | null> {
  const model = await apiFetch<InstallJobModel>("security/install");
  return model.job ?? null;
}

/** `POST /api/v1/security/install` — start a row's Install or Update, or
 *  Install & scan, with the consent dialog's answers; or join the install
 *  running. Nothing is downloaded until every line it needs is answered: until
 *  then the answer is the plan. */
export async function startInstall(ask: SecurityInstallAsk): Promise<InstallStart> {
  const res = await apiMutate(withMemberScope(INSTALL_PATH), {
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(ask),
    credentials: "same-origin",
  });
  if (res.status === 409) {
    const body = (await res.json()) as Partial<InstallConsentRequired>;
    if (body.plan) return { kind: "consent", plan: body.plan };
    // A scan running for the member: refused, not a question.
    throw new SecurityRequestError(res.status, body.error ?? "");
  }
  if (!res.ok) throw new SecurityRequestError(res.status, await apiErrorDetail(res));
  return { kind: "started", started: (await res.json()) as InstallJobStarted };
}

/** `POST /api/v1/security/install/cancel` — cancel the running install: the
 *  download is stopped, no partial tool is left, and the job's last step names
 *  what was installed. */
export async function cancelInstall(): Promise<InstallJob | null> {
  return cancelJob<InstallJob>(INSTALL_PATH);
}

/** `POST <path>/cancel` — the cancel both job kinds share: the job as it
 *  stands after the cancel, `null` when none ran. */
async function cancelJob<J>(path: string): Promise<J | null> {
  const res = await apiMutate(withMemberScope(`${path}/cancel`), { credentials: "same-origin" });
  if (!res.ok) throw new SecurityRequestError(res.status, await apiErrorDetail(res));
  return ((await res.json()) as { job: J | null }).job ?? null;
}

/** `POST /api/v1/security/suppress` — add a suppression to the committed ignore
 *  file: one finding's fingerprint, everything under a path (S-770), or a rule of
 *  one scanner in every file (S-814). `reason`
 *  is required, `expires` (`YYYY-MM-DD`) is the last day it suppresses, `null`
 *  until removed. The server writes a path in its canonical spelling. A refusal
 *  (a blank reason, a past expiry, a whole-project glob, a hand-spelled file)
 *  throws with the server's reason. */
export async function suppressFinding(
  selector: SecuritySelector,
  reason: string,
  expires: string | null,
  evidence: readonly string[] = [],
): Promise<SecurityIgnoreWrite> {
  // `evidence` is sent only when given, so a suppression without it is the
  // request it always was.
  return triage("/api/v1/security/suppress", { ...selector, reason, expires, ...(evidence.length > 0 ? { evidence } : {}) });
}

/** `POST /api/v1/security/reconfirm` — re-record the evidence hashes of the entry
 *  that suppresses the finding with this `fingerprint`, keeping its reason
 *  (FR-SC-32, S-818). A refusal (no evidence, an evidence symbol that no longer
 *  exists, an index behind its file) throws with the server's reason. */
export async function reconfirmFinding(fingerprint: string): Promise<SecurityIgnoreWrite> {
  return triage("/api/v1/security/reconfirm", { fingerprint });
}

/** `POST /api/v1/security/unsuppress` — remove the entry `selector` names: a
 *  finding's `suppression.by`, or an entry's own selector. */
export async function unsuppressFinding(selector: SecuritySelector): Promise<SecurityIgnoreWrite> {
  return triage("/api/v1/security/unsuppress", selector);
}

async function triage(path: string, body: object): Promise<SecurityIgnoreWrite> {
  const res = await apiMutate(withMemberScope(path), {
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
    credentials: "same-origin",
  });
  if (!res.ok) throw new SecurityRequestError(res.status, await apiErrorDetail(res));
  return (await res.json()) as SecurityIgnoreWrite;
}
