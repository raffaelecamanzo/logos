import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { StatsInfo } from "../api/types.ts";
import { Sidebar } from "./Sidebar.tsx";

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
