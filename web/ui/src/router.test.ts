import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { navigate, redirect, useNavigationState } from "./router.tsx";

afterEach(() => {
  vi.restoreAllMocks();
  window.history.replaceState(null, "", "/");
});

describe("redirect (S-194)", () => {
  it("uses replaceState — no extra back-stack entry", () => {
    const replaceSpy = vi.spyOn(window.history, "replaceState");
    const dispatchSpy = vi.spyOn(window, "dispatchEvent");
    redirect("/");
    expect(replaceSpy).toHaveBeenCalledWith({}, "", "/");
    expect(dispatchSpy).toHaveBeenCalledWith(expect.any(PopStateEvent));
  });

  it("does NOT call pushState (must not add a history entry)", () => {
    const pushSpy = vi.spyOn(window.history, "pushState");
    vi.spyOn(window.history, "replaceState").mockReturnValue(undefined);
    vi.spyOn(window, "dispatchEvent").mockReturnValue(true);
    redirect("/");
    expect(pushSpy).not.toHaveBeenCalled();
  });
});

describe("navigate", () => {
  it("uses pushState — adds a history entry", () => {
    const pushSpy = vi.spyOn(window.history, "pushState");
    const dispatchSpy = vi.spyOn(window, "dispatchEvent");
    navigate("/health");
    expect(pushSpy).toHaveBeenCalledWith({}, "", "/health");
    expect(dispatchSpy).toHaveBeenCalledWith(expect.any(PopStateEvent));
  });

  it("carries an explicit state payload into history.state (FR-WK-28)", () => {
    const pushSpy = vi.spyOn(window.history, "pushState");
    navigate("/wiki/page/overview/architecture", { q: "sandbox" });
    expect(pushSpy).toHaveBeenCalledWith({ q: "sandbox" }, "", "/wiki/page/overview/architecture");
    expect(window.history.state).toEqual({ q: "sandbox" });
  });
});

describe("useNavigationState (FR-WK-28)", () => {
  it("reads the current history.state on mount", () => {
    window.history.pushState({ q: "term" }, "", "/wiki/page/x");
    const { result } = renderHook(() => useNavigationState<{ q?: string }>());
    expect(result.current).toEqual({ q: "term" });
  });

  it("returns null when the current history entry carries no state", () => {
    window.history.pushState(null, "", "/wiki/page/x");
    const { result } = renderHook(() => useNavigationState());
    expect(result.current).toBeNull();
  });

  it("updates when a popstate event fires (navigate/back/forward)", () => {
    window.history.pushState({ q: "first" }, "", "/wiki/page/x");
    const { result } = renderHook(() => useNavigationState<{ q?: string }>());
    expect(result.current).toEqual({ q: "first" });

    act(() => {
      navigate("/wiki/page/y", { q: "second" });
    });
    expect(result.current).toEqual({ q: "second" });
  });
});
