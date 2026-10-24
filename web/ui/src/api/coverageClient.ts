/*
 * The Coverage page's Run coverage client (S-739, FR-CV-15, FR-CV-13; NFR-SE-06)
 * over the `coverage-run` job routes S-738 added.
 *
 * The read (`fetchCoverageRunJob`) is a GET like every other view's, and so is the
 * event stream (`followCoverageRun`): `GET /api/v1/coverage/run` asked for
 * `text/event-stream`, which a browser `EventSource` asks for. Start and cancel go
 * through the {@link apiMutate} seam, which echoes the per-session intent token
 * the server's same-origin/intent guard requires — the same path the Security
 * page's scan takes (`securityClient.ts`). Every request carries the selected
 * member (`?repo=`) in workspace mode, so the run starts, is followed and is
 * cancelled on the member the page shows.
 *
 * A start the project's execution consent does not cover is not an error: the
 * server answers `409` with what the dialog shows — the tools, and the custom
 * command both exactly (to echo back) and escaped (to display) — and starts
 * nothing.
 *
 * S-750 (FR-CV-17, FR-CV-18): the tool status widget's Install and Add
 * dependency start the same job through `startCoveragePrepared`, the install or
 * the addition first. Their `409` is the same question with the install lines and
 * the dependency's exact command and files beside it; a dependency Logos will not
 * add here is a refusal (`422`) naming the command to run by hand.
 */

import { apiMutate } from "../intent.ts";
import { withMemberScope } from "../workspace/scope.ts";
import { apiFetch, apiUrl } from "./client.ts";
import { apiErrorDetail } from "./configClient.ts";
// The types come through the barrel, as the Coverage views take them: `types.ts`
// is at the architecture rule's fan-in cap, and a type-only import adds no
// runtime cycle should the barrel ever re-export this client.
import type {
  CoverageConsentRequired,
  CoverageJob,
  CoverageJobEvent,
  CoverageJobModel,
  CoverageJobStarted,
  CoveragePrepareAsk,
  CoveragePrepareRequired,
  CoverageRunAsk,
} from "./index.ts";

const RUN_PATH = "/api/v1/coverage/run";

/** What a prepared start readies before the run: the route it posts to. */
export type CoveragePrepare = "install" | "dependency";

/** A start or cancel the server rejected. Carries the status and the server's
 *  verbatim detail, so the page states the real fault. */
export class CoverageRequestError extends Error {
  constructor(
    public readonly status: number,
    public readonly detail: string,
  ) {
    super(`the coverage request failed (HTTP ${status})`);
    this.name = "CoverageRequestError";
  }
}

/** What a failed coverage request says: the server's detail for a refusal, else
 *  the error's own message (a network fault). */
export function coverageRequestDetail(e: unknown): string {
  if (e instanceof CoverageRequestError) return e.detail || `HTTP ${e.status}`;
  return e instanceof Error ? e.message : String(e);
}

/** What a start resolved to: a running job (new or joined), or what the
 *  execution consent still lacks — for a prepared start, with the install lines
 *  and the dependency beside it. */
export type CoverageRunStart =
  | { kind: "started"; started: CoverageJobStarted }
  | { kind: "consent"; required: CoverageConsentRequired | CoveragePrepareRequired };

/** `GET /api/v1/coverage/run` — the member's run, going or last run, with every
 *  event it reported so far; `null` when none ran in this serve. */
export async function fetchCoverageRunJob(): Promise<CoverageJob | null> {
  const model = await apiFetch<CoverageJobModel>("coverage/run");
  return model.job ?? null;
}

/** `POST /api/v1/coverage/run` — start the run of every enabled, installed tool,
 *  or join the one going. `ask` is the consent dialog's answer: the tools it
 *  showed and the custom command's exact text; the server records exactly what
 *  was pending, outside the repository, and answers `409` again when the answer
 *  does not cover it. */
export async function startCoverageRun(ask: CoverageRunAsk = { consent: [] }): Promise<CoverageRunStart> {
  const res = await apiMutate(withMemberScope(RUN_PATH), {
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(ask),
    credentials: "same-origin",
  });
  return startAnswer(res);
}

/** `POST /api/v1/coverage/install` or `/dependency` — install the missing
 *  managed coverage tools, or add `ask.tool`'s missing coverage dev dependency,
 *  then run coverage, as the member's job. `ask` is the dialog's answer: the
 *  execution consent as Run coverage sends it, the install lines and the
 *  dependency commands exactly as shown. Nothing is recorded or started until
 *  it accepts every line; a run already going is a refusal, never joined. */
export async function startCoveragePrepared(prepare: CoveragePrepare, ask: CoveragePrepareAsk): Promise<CoverageRunStart> {
  const res = await apiMutate(withMemberScope(`/api/v1/coverage/${prepare}`), {
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(ask),
    credentials: "same-origin",
  });
  return startAnswer(res);
}

/** What a start's response says: the job; or, for a `409` naming what the
 *  consent lacks, the dialog's question; any other refusal throws with the
 *  server's words. */
async function startAnswer(res: Response): Promise<CoverageRunStart> {
  if (res.ok) return { kind: "started", started: (await res.json()) as CoverageJobStarted };
  if (res.status !== 409) throw new CoverageRequestError(res.status, await apiErrorDetail(res));
  const body = (await res.json()) as Partial<CoverageConsentRequired>;
  if (!body.consent_required) throw new CoverageRequestError(res.status, body.error ?? "");
  return { kind: "consent", required: body as CoverageConsentRequired };
}

/** `POST /api/v1/coverage/run/cancel` — cancel the run: the tool going is killed
 *  with its whole process group and no tool starts after. The answer is the job
 *  as it stood when the cancel landed (still `running` for an instant); `null`
 *  when none ran. */
export async function cancelCoverageRun(): Promise<CoverageJob | null> {
  const res = await apiMutate(withMemberScope(`${RUN_PATH}/cancel`), { credentials: "same-origin" });
  if (!res.ok) throw new CoverageRequestError(res.status, await apiErrorDetail(res));
  return ((await res.json()) as CoverageJobModel).job ?? null;
}

/** What a followed run reports: each `step` as it happens (the replay first),
 *  then its `end` once — the job as it ended, without its events. `lost` means
 *  the stream broke before the end: nothing more arrives on it. */
export interface CoverageRunFollower {
  step: (event: CoverageJobEvent) => void;
  end: (job: CoverageJob | null) => void;
  lost: () => void;
}

/** Follow the member's run over same-origin Server-Sent Events. The stream
 *  follows the run going when it opens, replays what it reported so far and
 *  closes after its `end`; with no run, the `end` carries `null` at once.
 *  Returns what stops following. The source is closed on the end and on a
 *  break, so the browser never reconnects on its own: a reconnect would replay
 *  whichever run is going then. */
export function followCoverageRun(follower: CoverageRunFollower): () => void {
  const source = new EventSource(apiUrl("coverage/run"));
  let open = true;
  const close = () => {
    open = false;
    source.close();
  };
  const read = (message: Event, use: (data: unknown) => void) => {
    if (!open) return;
    let data: unknown;
    try {
      data = JSON.parse((message as MessageEvent<string>).data);
    } catch {
      close();
      follower.lost();
      return;
    }
    use(data);
  };
  source.addEventListener("step", (m) => read(m, (data) => follower.step(data as CoverageJobEvent)));
  source.addEventListener("end", (m) =>
    read(m, (data) => {
      close();
      follower.end((data as CoverageJobModel).job ?? null);
    }),
  );
  source.addEventListener("error", () => {
    if (!open) return;
    close();
    follower.lost();
  });
  return close;
}
