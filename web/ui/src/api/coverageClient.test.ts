// The Run coverage client (S-739, FR-CV-15, FR-CV-13, NFR-SE-06): in workspace
// mode every request carries the selected member, so the run starts, is followed
// and is cancelled on the member the page shows; the writes carry the intent
// token; a `409` with what the consent lacks is the dialog's question, not an
// error; any other refusal throws with the server's status and detail. The event
// stream is closed on its end and on a break, so the browser never reconnects to
// whichever run goes then.
import { afterEach, describe, expect, it, vi } from "vitest";

import { installEventSources } from "../test/eventSource.ts";
import { CONSENT_FIRST, COMPLETED_END, DEPENDENCY_FIRST, EVENTS, RUNNING } from "../views/analytics/fixtures/coverageRun.ts";
import { setScopedMember } from "../workspace/scope.ts";
import {
  CoverageRequestError,
  cancelCoverageRun,
  coverageRequestDetail,
  fetchCoverageRunJob,
  followCoverageRun,
  startCoveragePrepared,
  startCoverageRun,
} from "./coverageClient.ts";
import type { CoverageJob, CoverageJobEvent } from "./types.ts";

interface Call {
  url: string;
  method: string;
  intent: string | null;
  body: string | null;
}

/** Stubs fetch answering every request with `status` and `body`. */
function stub(status: number, body: unknown): Call[] {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string, init: RequestInit = {}) => {
      calls.push({
        url,
        method: init.method ?? "GET",
        intent: new Headers(init.headers).get("x-logos-intent"),
        body: typeof init.body === "string" ? init.body : null,
      });
      return Promise.resolve({
        ok: status >= 200 && status < 300,
        status,
        json: () => Promise.resolve(body),
        text: () => Promise.resolve(typeof body === "string" ? body : JSON.stringify(body)),
      } as Response);
    }),
  );
  return calls;
}

afterEach(() => {
  setScopedMember(null);
  vi.unstubAllGlobals();
});

describe("the Run coverage client (S-739)", () => {
  it("reads the member's run, and null when none ran in this serve", async () => {
    const calls = stub(200, { job: RUNNING });
    expect(await fetchCoverageRunJob()).toEqual(RUNNING);
    expect(calls[0]).toMatchObject({ url: "/api/v1/coverage/run", method: "GET" });
    stub(200, { job: null });
    expect(await fetchCoverageRunJob()).toBeNull();
  });

  it("starts with the intent token and the dialog's answer, and reads a join", async () => {
    const calls = stub(200, { joined: true, job: RUNNING });
    const answer = await startCoverageRun({ consent: ["go", "custom"], refresh_cmd: "make cov" });
    expect(answer).toEqual({ kind: "started", started: { joined: true, job: RUNNING } });
    expect(calls[0]).toMatchObject({ url: "/api/v1/coverage/run", method: "POST", intent: "test-intent-token" });
    expect(JSON.parse(calls[0].body ?? "")).toEqual({ consent: ["go", "custom"], refresh_cmd: "make cov" });
  });

  it("sends an empty consent by default", async () => {
    const calls = stub(200, { joined: false, job: RUNNING });
    await startCoverageRun();
    expect(JSON.parse(calls[0].body ?? "")).toEqual({ consent: [] });
  });

  it("returns the consent question of a 409, not an error", async () => {
    stub(409, CONSENT_FIRST);
    expect(await startCoverageRun()).toEqual({ kind: "consent", required: CONSENT_FIRST });
  });

  it("throws a 409 without a consent question with the server's words", async () => {
    stub(409, { error: "a scan holds the project" });
    const refused = await startCoverageRun().catch((e: unknown) => e);
    expect(refused).toBeInstanceOf(CoverageRequestError);
    expect(coverageRequestDetail(refused)).toBe("a scan holds the project");
  });

  it("throws any other refusal with its status and detail", async () => {
    stub(403, { error: "the request carries no valid intent token" });
    const refused = (await startCoverageRun().catch((e: unknown) => e)) as CoverageRequestError;
    expect(refused.status).toBe(403);
    expect(coverageRequestDetail(refused)).toBe("the request carries no valid intent token");
    expect(coverageRequestDetail(new Error("offline"))).toBe("offline");
  });

  it("states a refusal that is not JSON verbatim", async () => {
    stub(502, "Bad Gateway");
    const refused = (await startCoverageRun().catch((e: unknown) => e)) as CoverageRequestError;
    expect(refused).toBeInstanceOf(CoverageRequestError);
    expect(coverageRequestDetail(refused)).toBe("Bad Gateway");
  });

  it("cancels with the intent token and answers the job as it stands", async () => {
    const calls = stub(200, { job: RUNNING });
    expect(await cancelCoverageRun()).toEqual(RUNNING);
    expect(calls[0]).toMatchObject({ url: "/api/v1/coverage/run/cancel", method: "POST", intent: "test-intent-token" });
    stub(500, { error: "boom" });
    await expect(cancelCoverageRun()).rejects.toBeInstanceOf(CoverageRequestError);
  });

  it("carries the selected member on every request, the stream included", async () => {
    setScopedMember("svc-a");
    const sources = installEventSources();
    const calls = stub(200, { job: null, joined: false });
    await fetchCoverageRunJob();
    await startCoverageRun();
    await cancelCoverageRun();
    followCoverageRun({ step: vi.fn(), end: vi.fn(), lost: vi.fn() });
    expect(calls.map((c) => c.url)).toEqual([
      "/api/v1/coverage/run?repo=svc-a",
      "/api/v1/coverage/run?repo=svc-a",
      "/api/v1/coverage/run/cancel?repo=svc-a",
    ]);
    expect(sources[0].url).toBe("/api/v1/coverage/run?repo=svc-a");
  });
});

describe("following a run over Server-Sent Events (S-739)", () => {
  it("hands each step on, then the end once, and closes the stream", () => {
    const sources = installEventSources();
    const steps: CoverageJobEvent[] = [];
    const ends: (CoverageJob | null)[] = [];
    const lost = vi.fn();
    followCoverageRun({ step: (e) => steps.push(e), end: (j) => ends.push(j), lost });
    const [source] = sources;
    expect(source.url).toBe("/api/v1/coverage/run");
    source.emit("step", EVENTS[0]);
    source.emit("step", EVENTS[1]);
    source.emit("end", { job: COMPLETED_END });
    expect(steps).toEqual([EVENTS[0], EVENTS[1]]);
    expect(ends).toEqual([COMPLETED_END]);
    expect(source.closed).toBe(true);
    // Nothing after the end is read; the closing error is not a break.
    source.emit("step", EVENTS[2]);
    source.fail();
    expect(steps).toHaveLength(2);
    expect(lost).not.toHaveBeenCalled();
  });

  it("reads an end with no run as null", () => {
    const sources = installEventSources();
    const end = vi.fn();
    followCoverageRun({ step: vi.fn(), end, lost: vi.fn() });
    sources[0].emit("end", { job: null });
    expect(end).toHaveBeenCalledWith(null);
  });

  it("closes a broken stream and says it was lost, once", () => {
    const sources = installEventSources();
    const lost = vi.fn();
    followCoverageRun({ step: vi.fn(), end: vi.fn(), lost });
    sources[0].fail();
    sources[0].fail();
    expect(sources[0].closed).toBe(true);
    expect(lost).toHaveBeenCalledTimes(1);
  });

  it("treats data that is not JSON as a break", () => {
    const sources = installEventSources();
    const step = vi.fn();
    const lost = vi.fn();
    followCoverageRun({ step, end: vi.fn(), lost });
    sources[0].emit("step", "{not json");
    expect(step).not.toHaveBeenCalled();
    expect(lost).toHaveBeenCalledTimes(1);
    expect(sources[0].closed).toBe(true);
  });

  it("stops following when asked", () => {
    const sources = installEventSources();
    const step = vi.fn();
    const stop = followCoverageRun({ step, end: vi.fn(), lost: vi.fn() });
    stop();
    expect(sources[0].closed).toBe(true);
    sources[0].emit("step", EVENTS[0]);
    expect(step).not.toHaveBeenCalled();
  });
});

describe("Install and Add dependency (S-750)", () => {
  it("posts each to its own route with the intent token and the dialog's whole answer", async () => {
    const calls = stub(200, { joined: false, job: RUNNING });
    const ask = { consent: ["vitest"], tool: "vitest", dependency: ["pnpm add -D @vitest/coverage-v8@3"] };
    expect(await startCoveragePrepared("dependency", ask)).toEqual({ kind: "started", started: { joined: false, job: RUNNING } });
    await startCoveragePrepared("install", { consent: [], install: ["cargo-llvm-cov 0.9.1"] });
    expect(calls.map((c) => [c.url, c.method, c.intent])).toEqual([
      ["/api/v1/coverage/dependency", "POST", "test-intent-token"],
      ["/api/v1/coverage/install", "POST", "test-intent-token"],
    ]);
    expect(JSON.parse(calls[0].body ?? "")).toEqual(ask);
  });

  it("returns a 409's question with the dependency beside the run's consent", async () => {
    stub(409, DEPENDENCY_FIRST);
    expect(await startCoveragePrepared("dependency", { consent: [], tool: "vitest" })).toEqual({ kind: "consent", required: DEPENDENCY_FIRST });
  });

  it("throws a dependency Logos will not add with the command to run by hand", async () => {
    const error = "no lockfile names this project's package manager. Nothing was changed. Run it by hand: npm install --save-dev @vitest/coverage-v8@3";
    stub(422, { error, hand_command: "npm install --save-dev @vitest/coverage-v8@3" });
    const refused = (await startCoveragePrepared("dependency", { consent: [], tool: "vitest" }).catch((e: unknown) => e)) as CoverageRequestError;
    expect(refused.status).toBe(422);
    expect(coverageRequestDetail(refused)).toBe(error);
  });

  it("carries the selected member", async () => {
    setScopedMember("svc-a");
    const calls = stub(200, { joined: false, job: RUNNING });
    await startCoveragePrepared("install", { consent: [] });
    expect(calls[0].url).toBe("/api/v1/coverage/install?repo=svc-a");
  });
});
