// The Run coverage job hook (S-739, FR-CV-15): a broken event stream is not the
// end of the run — the page says so, reads the run again and, while it still
// goes, follows it again, its replay merging by `seq`; an answer that left the
// server before the run ended never rolls an ended run back to running; and a
// run this page followed reloads the page once when it ends, never one it only
// found ended. HF-6: that run is shown ended only once the reload has settled,
// whichever way its end arrives, so the page never says it finished over the
// figures it read before the run.
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { CoverageJob } from "../../api/types.ts";
import { COVERAGE_RUN_TEXT } from "../../copy/coverageView.copy.ts";
import { installEventSources } from "../../test/eventSource.ts";
import { useCoverageRun } from "./CoverageRunPanel.tsx";
import {
  COMPLETED,
  COMPLETED_END,
  DEPENDENCY_FIRST,
  EVENTS,
  INSTALL_FIRST,
  PREPARABLE_TOOLS,
  RUNNING,
  TOOLS,
} from "./fixtures/coverageRun.ts";

/** Stubs fetch: each `GET /api/v1/coverage/run` answers the next of `reads`
 *  (the last one again) — an `Error` is a failed read; each POST answers the
 *  next of `posts`. */
function stub(reads: (CoverageJob | null | Error)[], posts: unknown[] = []) {
  const gets = vi.fn();
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string, init: RequestInit = {}) => {
      let body: unknown;
      if ((init.method ?? "GET") === "POST") body = posts.shift();
      else {
        gets(url);
        const read = reads.length > 1 ? reads.shift() : reads[0];
        if (read instanceof Error) return Promise.reject(read);
        body = { job: read };
      }
      return Promise.resolve({ ok: true, status: 200, json: () => Promise.resolve(structuredClone(body)) } as Response);
    }),
  );
  return gets;
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function mount(onFinished = vi.fn()) {
  const hook = renderHook(() => useCoverageRun({ tools: TOOLS, consentedBefore: false, onFinished, retryMs: 5 }));
  return { hook, onFinished };
}

describe("useCoverageRun (S-739)", () => {
  it("reads the run again after a broken stream and follows it again while it goes", async () => {
    const sources = installEventSources();
    const gets = stub([{ ...RUNNING, events: EVENTS.slice(0, 2) }, RUNNING]);
    const { hook } = mount();
    await waitFor(() => expect(sources).toHaveLength(1));
    act(() => sources[0].fail());
    expect(hook.result.current.alert).toBe(COVERAGE_RUN_TEXT.lost);
    await waitFor(() => expect(sources).toHaveLength(2));
    expect(gets).toHaveBeenCalledTimes(2);
    expect(hook.result.current.alert).toBeNull();
    // The replay of the second stream merges with what was seen, each step once.
    act(() => {
      for (const step of EVENTS.slice(0, 9)) sources[1].emit("step", step);
    });
    expect(hook.result.current.view.steps.map((s) => s.seq)).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8]);
  });

  it("does not follow again a run that ended while the stream was broken, and reloads once", async () => {
    const sources = installEventSources();
    stub([RUNNING, COMPLETED]);
    const { hook, onFinished } = mount();
    await waitFor(() => expect(sources).toHaveLength(1));
    act(() => sources[0].fail());
    await waitFor(() => expect(hook.result.current.view.job?.state).toBe("completed"));
    expect(sources).toHaveLength(1);
    expect(onFinished).toHaveBeenCalledTimes(1);
  });

  it("never rolls an ended run back to running", async () => {
    const sources = installEventSources();
    stub([RUNNING], [{ job: RUNNING }]);
    const { hook, onFinished } = mount();
    await waitFor(() => expect(sources).toHaveLength(1));
    act(() => sources[0].emit("end", { job: COMPLETED_END }));
    await waitFor(() => expect(hook.result.current.view.job?.state).toBe("completed"));
    // A cancel answered with the job as it stood a moment before it ended.
    await act(async () => hook.result.current.cancel());
    expect(hook.result.current.view.job?.state).toBe("completed");
    expect(hook.result.current.running).toBe(false);
    expect(sources).toHaveLength(1);
    expect(onFinished).toHaveBeenCalledTimes(1);
  });

  it("credits a stream that connected to the next run to that run, and reloads once (review F1)", async () => {
    const sources = installEventSources();
    const next = { ...COMPLETED, id: 5 };
    // Mount reads run 4 going; by the time the stream connects, run 5 ran instead.
    stub([RUNNING, next]);
    const { hook, onFinished } = mount();
    await waitFor(() => expect(sources).toHaveLength(1));
    act(() => {
      for (const step of EVENTS) sources[0].emit("step", step);
      sources[0].emit("end", { job: { ...COMPLETED_END, id: 5 } });
    });
    await waitFor(() => expect(onFinished).toHaveBeenCalledTimes(1));
    expect(hook.result.current.view.job?.id).toBe(5);
    expect(hook.result.current.rows.map((r) => `${r.tool}:${r.state}`)).toEqual([
      "vitest:tests_failed",
      "maven:not_installed",
      "go:ran",
      "custom:failed",
    ]);
  });

  it("reloads once for a run a re-read finds ended in the followed run's place (review F1)", async () => {
    const sources = installEventSources();
    stub([RUNNING, { ...COMPLETED, id: 5 }]);
    const { hook, onFinished } = mount();
    await waitFor(() => expect(sources).toHaveLength(1));
    act(() => sources[0].fail());
    await waitFor(() => expect(hook.result.current.view.job?.id).toBe(5));
    await waitFor(() => expect(onFinished).toHaveBeenCalledTimes(1));
  });

  it("clears the broken-stream alert once the run is no longer followed (review F2)", async () => {
    const sources = installEventSources();
    stub([RUNNING], [{ job: COMPLETED_END }]);
    const { result } = renderHook(() => useCoverageRun({ tools: TOOLS, consentedBefore: false, onFinished: vi.fn(), retryMs: 50 }));
    await waitFor(() => expect(sources).toHaveLength(1));
    act(() => sources[0].fail());
    expect(result.current.alert).toBe(COVERAGE_RUN_TEXT.lost);
    // A cancel answered with the run already ended, inside the re-read's wait.
    await act(async () => result.current.cancel());
    expect(result.current.view.job?.state).toBe("completed");
    await new Promise((done) => setTimeout(done, 120));
    expect(result.current.alert).toBeNull();
    expect(sources).toHaveLength(1);
  });

  it("reads the run again after a failed re-read, and follows it again (review)", async () => {
    const sources = installEventSources();
    const gets = stub([RUNNING, new Error("offline"), RUNNING]);
    const { hook } = mount();
    await waitFor(() => expect(sources).toHaveLength(1));
    act(() => sources[0].fail());
    await waitFor(() => expect(sources).toHaveLength(2));
    expect(gets).toHaveBeenCalledTimes(3);
    expect(hook.result.current.alert).toBeNull();
  });

  it("stops following once the page is left: no re-read, no new stream (review)", async () => {
    const sources = installEventSources();
    const gets = stub([RUNNING]);
    const { hook } = mount();
    await waitFor(() => expect(sources).toHaveLength(1));
    act(() => sources[0].fail());
    hook.unmount();
    await new Promise((done) => setTimeout(done, 60));
    expect(gets).toHaveBeenCalledTimes(1);
    expect(sources).toHaveLength(1);
  });

  it("does not reload for a run it only found ended", async () => {
    installEventSources();
    stub([COMPLETED]);
    const { hook, onFinished } = mount();
    await waitFor(() => expect(hook.result.current.view.job?.state).toBe("completed"));
    expect(onFinished).not.toHaveBeenCalled();
    expect(hook.result.current.rows.map((r) => r.state)).toEqual(["tests_failed", "not_installed", "ran", "failed"]);
  });

  it("can run only while a tool here is installed and enabled", () => {
    stub([null]);
    const none = TOOLS.map((t) => (t.state === "never_run" ? { ...t, state: "not_installed" as const } : t));
    const { result } = renderHook(() => useCoverageRun({ tools: none, consentedBefore: false, onFinished: vi.fn() }));
    expect(result.current.canRun).toBe(false);
  });
});

describe("useCoverageRun's Install and Add dependency (S-750)", () => {
  /** Stubs fetch: every GET reads no run; each POST answers the next of
   *  `posts` (`[status, body]`) and is recorded with its route and body. */
  function stubPosts(posts: [number, unknown][]) {
    const sent: { url: string; body: unknown }[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string, init: RequestInit = {}) => {
        if ((init.method ?? "GET") !== "POST") {
          return Promise.resolve({ ok: true, status: 200, json: () => Promise.resolve({ job: null }) } as Response);
        }
        sent.push({ url, body: JSON.parse(String(init.body)) });
        const [status, body] = posts.shift() ?? [500, { error: "unexpected" }];
        return Promise.resolve({
          ok: status < 300,
          status,
          json: () => Promise.resolve(structuredClone(body)),
          text: () => Promise.resolve(JSON.stringify(body)),
        } as Response);
      }),
    );
    return sent;
  }

  const prepared = () => renderHook(() => useCoverageRun({ tools: PREPARABLE_TOOLS, consentedBefore: false, onFinished: vi.fn() }));

  it("asks before adding, then sends back exactly the command and tool the dialog showed", async () => {
    installEventSources();
    const sent = stubPosts([[409, DEPENDENCY_FIRST], [200, { joined: false, job: RUNNING }]]);
    const { result } = prepared();
    await act(async () => result.current.addDependency("vitest"));
    expect(sent).toEqual([{ url: "/api/v1/coverage/dependency", body: { consent: [], tool: "vitest" } }]);
    expect(result.current.consent.required).toEqual(DEPENDENCY_FIRST);
    expect(result.current.consent.action).toEqual({ kind: "dependency", tool: "vitest" });
    expect(result.current.consent.willRun).toEqual(["vitest"]);
    await act(async () => result.current.consent.accept());
    expect(sent[1]).toEqual({
      url: "/api/v1/coverage/dependency",
      body: { consent: ["vitest"], install: [], dependency: ["pnpm add -D @vitest/coverage-v8@3"], tool: "vitest" },
    });
    expect(result.current.consent.required).toBeNull();
    expect(result.current.view.job?.state).toBe("running");
  });

  it("accepts an Install with each line's echo, and a decline sends nothing more", async () => {
    installEventSources();
    const sent = stubPosts([[409, INSTALL_FIRST], [409, INSTALL_FIRST], [200, { joined: false, job: RUNNING }]]);
    const { result } = prepared();
    await act(async () => result.current.install());
    expect(result.current.consent.willRun).toEqual(["cargo-llvm-cov"]);
    act(() => result.current.consent.decline());
    expect(result.current.consent.required).toBeNull();
    expect(sent).toHaveLength(1);
    await act(async () => result.current.install());
    await act(async () => result.current.consent.accept());
    expect(sent.map((s) => s.url)).toEqual(["/api/v1/coverage/install", "/api/v1/coverage/install", "/api/v1/coverage/install"]);
    expect(sent[2].body).toEqual({
      consent: ["cargo-llvm-cov"],
      install: ["cargo-llvm-cov 0.9.1", "rustup component add llvm-tools-preview"],
      dependency: [],
    });
  });

  it("states a dependency Logos will not add, with the command to run by hand", async () => {
    installEventSources();
    const error = "no lockfile names this project's package manager. Nothing was changed. Run it by hand: npm install --save-dev @vitest/coverage-v8@3";
    stubPosts([[422, { error, hand_command: "npm install --save-dev @vitest/coverage-v8@3" }]]);
    const { result } = prepared();
    await act(async () => result.current.addDependency("vitest"));
    expect(result.current.alert).toBe(COVERAGE_RUN_TEXT.requestFailed(error));
    expect(result.current.consent.required).toBeNull();
  });

  it("keeps a plain Run on its own route", async () => {
    installEventSources();
    const sent = stubPosts([[200, { joined: false, job: RUNNING }]]);
    const { result } = prepared();
    await act(async () => result.current.run());
    expect(sent).toEqual([{ url: "/api/v1/coverage/run", body: { consent: [] } }]);
  });
});

/** A promise held until `release`, standing for a reload that has not answered. */
function held() {
  let release = () => undefined as void;
  const settled = new Promise<void>((done) => {
    release = done;
  });
  return { settled, release };
}

const wait = (ms: number) => new Promise((done) => setTimeout(done, ms));

describe("useCoverageRun shows a run ended only once the page's reload has settled (HF-6)", () => {
  it("keeps the run running while onFinished is pending, then ends it, reloading once", async () => {
    const sources = installEventSources();
    stub([RUNNING]);
    const reload = held();
    const { hook, onFinished } = mount(vi.fn(() => reload.settled));
    await waitFor(() => expect(sources).toHaveLength(1));
    act(() => {
      for (const step of EVENTS) sources[0].emit("step", step);
      sources[0].emit("end", { job: COMPLETED_END });
    });
    await waitFor(() => expect(onFinished).toHaveBeenCalledTimes(1));
    await wait(50);
    expect(hook.result.current.view.job?.state).toBe("running");
    expect(hook.result.current.running).toBe(true);
    reload.release();
    await waitFor(() => expect(hook.result.current.view.job?.state).toBe("completed"));
    expect(hook.result.current.running).toBe(false);
    expect(onFinished).toHaveBeenCalledTimes(1);
  });

  it("holds a cancel answer that is first to see the end, and reads of the ending run add nothing", async () => {
    const sources = installEventSources();
    stub([RUNNING], [{ job: COMPLETED_END }, { job: RUNNING }, { job: COMPLETED_END }]);
    const reload = held();
    const { hook, onFinished } = mount(vi.fn(() => reload.settled));
    await waitFor(() => expect(sources).toHaveLength(1));
    await act(async () => hook.result.current.cancel());
    expect(onFinished).toHaveBeenCalledTimes(1);
    expect(hook.result.current.view.job?.state).toBe("running");
    // A late cancel answer that still says running, and one that says ended again.
    await act(async () => hook.result.current.cancel());
    await act(async () => hook.result.current.cancel());
    expect(hook.result.current.view.job?.state).toBe("running");
    expect(onFinished).toHaveBeenCalledTimes(1);
    reload.release();
    await waitFor(() => expect(hook.result.current.view.job?.state).toBe("completed"));
    expect(onFinished).toHaveBeenCalledTimes(1);
  });

  it("never lets the late end replace a run started during the wait", async () => {
    const sources = installEventSources();
    const next = { ...RUNNING, id: 5, events: [] };
    stub([RUNNING], [{ joined: false, job: next }]);
    const reload = held();
    const { hook, onFinished } = mount(vi.fn(() => reload.settled));
    await waitFor(() => expect(sources).toHaveLength(1));
    act(() => sources[0].emit("end", { job: COMPLETED_END }));
    await waitFor(() => expect(onFinished).toHaveBeenCalledTimes(1));
    await act(async () => hook.result.current.run());
    expect(hook.result.current.view.job?.id).toBe(5);
    reload.release();
    await wait(50);
    expect(hook.result.current.view.job).toMatchObject({ id: 5, state: "running" });
    expect(hook.result.current.running).toBe(true);
    expect(onFinished).toHaveBeenCalledTimes(1);
  });

  it("keeps an earlier run's settled reload from releasing a later run's end (review)", async () => {
    const sources = installEventSources();
    const next = { ...RUNNING, id: 5, events: [] };
    const ended = { ...COMPLETED_END, id: 5 };
    stub([RUNNING], [{ joined: false, job: next }, { job: ended }]);
    const first = held();
    const second = held();
    const onFinished = vi.fn().mockReturnValueOnce(first.settled).mockReturnValueOnce(second.settled);
    const { hook } = mount(onFinished);
    await waitFor(() => expect(sources).toHaveLength(1));
    act(() => sources[0].emit("end", { job: COMPLETED_END }));
    await waitFor(() => expect(onFinished).toHaveBeenCalledTimes(1));
    await act(async () => hook.result.current.run());
    await waitFor(() => expect(sources).toHaveLength(2));
    act(() => sources[1].emit("end", { job: ended }));
    await waitFor(() => expect(onFinished).toHaveBeenCalledTimes(2));
    first.release();
    await wait(30);
    // Run 4's reload settled; run 5's has not, so a read of run 5 ended adds nothing.
    await act(async () => hook.result.current.cancel());
    expect(hook.result.current.view.job).toMatchObject({ id: 5, state: "running" });
    second.release();
    await waitFor(() => expect(hook.result.current.view.job?.state).toBe("completed"));
    expect(onFinished).toHaveBeenCalledTimes(2);
  });

  it("reloads once for a run's end, however many reads of the ended run follow (review)", async () => {
    const sources = installEventSources();
    stub([RUNNING], [{ job: COMPLETED_END }]);
    const { hook, onFinished } = mount();
    await waitFor(() => expect(sources).toHaveLength(1));
    act(() => sources[0].emit("end", { job: COMPLETED_END }));
    await waitFor(() => expect(hook.result.current.view.job?.state).toBe("completed"));
    await act(async () => hook.result.current.cancel());
    await wait(30);
    expect(hook.result.current.view.job?.state).toBe("completed");
    expect(onFinished).toHaveBeenCalledTimes(1);
  });

  it("never lets a slow pickup of a run found at mount replace a newer run this page started (review)", async () => {
    const sources = installEventSources();
    const next = { ...RUNNING, id: 5, events: [] };
    const pickup = held();
    const reply = (body: unknown) =>
      ({ ok: true, status: 200, json: () => Promise.resolve(structuredClone(body)) }) as Response;
    vi.stubGlobal(
      "fetch",
      vi.fn((_url: string, init: RequestInit = {}) =>
        (init.method ?? "GET") === "POST"
          ? Promise.resolve(reply({ joined: false, job: next }))
          : pickup.settled.then(() => reply({ job: RUNNING })),
      ),
    );
    const { hook } = mount();
    await act(async () => hook.result.current.run());
    await waitFor(() => expect(hook.result.current.view.job?.id).toBe(5));
    await waitFor(() => expect(sources).toHaveLength(1));
    pickup.release();
    await wait(30);
    expect(hook.result.current.view.job).toMatchObject({ id: 5, state: "running" });
  });

  it.each([
    ["a reload that fails", () => Promise.reject(new Error("offline"))],
    ["a reload that throws", () => { throw new Error("offline"); }],
  ])("still ends the run after %s, with nothing unhandled", async (_name, onFinished) => {
    const sources = installEventSources();
    stub([RUNNING]);
    const { hook } = mount(vi.fn(onFinished));
    await waitFor(() => expect(sources).toHaveLength(1));
    act(() => sources[0].emit("end", { job: COMPLETED_END }));
    await waitFor(() => expect(hook.result.current.view.job?.state).toBe("completed"));
    expect(hook.result.current.running).toBe(false);
  });

  it("publishes nothing once the page is left mid-wait", async () => {
    const sources = installEventSources();
    stub([RUNNING]);
    const reload = held();
    const { hook, onFinished } = mount(vi.fn(() => reload.settled));
    await waitFor(() => expect(sources).toHaveLength(1));
    act(() => sources[0].emit("end", { job: COMPLETED_END }));
    await waitFor(() => expect(onFinished).toHaveBeenCalledTimes(1));
    hook.unmount();
    reload.release();
    await wait(30);
    expect(hook.result.current.view.job?.state).toBe("running");
    expect(onFinished).toHaveBeenCalledTimes(1);
  });
});
