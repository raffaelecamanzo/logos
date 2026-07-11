import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { StatsInfo } from "../api/types.ts";
import { Sidebar } from "./Sidebar.tsx";
import { WorkspaceProvider } from "../workspace/WorkspaceContext.tsx";

vi.mock("../router.tsx", () => ({ navigate: vi.fn() }));

function stats(callsTotal: number): StatsInfo {
  return {
    window_days: 7,
    calls_total: callsTotal,
    calls_by_tool: [],
    latency_p50_ms: 0,
    latency_p95_ms: 0,
    latency_p99_ms: 0,
    reads_saved_estimate: 0,
    tokens_saved_estimate: 0,
    artifact_bindings: {},
    activity_by_day: [],
    calls_by_origin: [],
    warnings: callsTotal === 0 ? ["no telemetry recorded yet"] : [],
  };
}

function stubStats(callsTotal: number) {
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.resolve({ ok: true, json: () => Promise.resolve(stats(callsTotal)) } as Response)),
  );
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("Sidebar — Statistics nav (S-235, FR-UI-27)", () => {
  it("renders a Statistics item directly above Config", () => {
    stubStats(5);
    render(<Sidebar pathname="/" />);
    const links = screen.getAllByRole("link").map((a) => a.textContent);
    const statsIdx = links.findIndex((t) => t?.includes("Statistics"));
    const configIdx = links.findIndex((t) => t?.includes("Config"));
    expect(statsIdx).toBeGreaterThanOrEqual(0);
    expect(configIdx).toBe(statsIdx + 1);
  });

  it("mutes the Statistics item when the telemetry store is empty (NFR-CC-04)", async () => {
    stubStats(0);
    render(<Sidebar pathname="/" />);
    const link = screen.getByRole("link", { name: /Statistics/ });
    await waitFor(() =>
      expect(link).toHaveAttribute("title", expect.stringMatching(/awaiting data/i)),
    );
  });

  it("does NOT mute the Statistics item when usage has been recorded", async () => {
    stubStats(5);
    render(<Sidebar pathname="/" />);
    const link = screen.getByRole("link", { name: /Statistics/ });
    // Wait until the probe has actually fired and settled — otherwise "no title"
    // could pass merely because the probe is still loading (loading ≠ populated).
    await waitFor(() => expect(fetch).toHaveBeenCalled());
    // Flush the resolved-promise microtask so the ready state has committed.
    await waitFor(() => expect(link).not.toHaveAttribute("title"));
  });
});

// ── S-250 / FR-UI-29 AC4: the workspace tab is workspace-mode ONLY ────────────

describe("Sidebar workspace gating (S-250)", () => {
  /** Render the sidebar inside a provider whose roster probe answers `probeStatus`. */
  function mountWithMode(probeStatus: number) {
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        const isProbe = url.startsWith("/api/v1/workspace/roster");
        return Promise.resolve({
          ok: !isProbe || probeStatus === 200,
          status: isProbe ? probeStatus : 200,
          json: () =>
            Promise.resolve(
              isProbe ? { workspace: "shop", default: "api", members: ["api", "web"] } : stats(1),
            ),
        } as Response);
      }),
    );
    return render(
      <WorkspaceProvider>
        <Sidebar pathname="/" />
      </WorkspaceProvider>,
    );
  }

  it("renders NO Workspace item in a single-root serve — the sidebar is unchanged", async () => {
    mountWithMode(404);
    // Wait for the probe to SETTLE before asserting absence; otherwise this passes
    // merely because the shell is still loading.
    await waitFor(() => expect(screen.getByRole("link", { name: /Dashboard/ })).toBeInTheDocument());
    await waitFor(() => expect(screen.queryByRole("link", { name: /^Workspace$/ })).toBeNull());
  });

  it("renders the Workspace item in workspace mode", async () => {
    mountWithMode(200);
    expect(await screen.findByRole("link", { name: /Workspace/ })).toHaveAttribute(
      "href",
      "/workspace",
    );
  });
});
