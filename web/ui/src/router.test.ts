import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { currentUrl, navigate, redirect, replaceUrl, useNavigationState } from "./router.tsx";
import { setScopedMember } from "./workspace/scope.ts";

afterEach(() => {
  vi.restoreAllMocks();
  window.history.replaceState(null, "", "/");
  setScopedMember(null);
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

// ── S-426 / FR-UI-35 / ADR-52: every entry this module writes names the member ──

describe("history writes carry the active member (S-426)", () => {
  it("pushes the member onto a navigation, so a tab change does not drop it", () => {
    // Without this, clicking Health while scoped to `web` would push a bare
    // `/health`, the popstate would re-read an unscoped URL, and the shell would
    // silently drop back to the DEFAULT member on an ordinary tab click.
    setScopedMember("web");
    navigate("/health");
    expect(window.location.pathname + window.location.search).toBe("/health?repo=web");
  });

  it("keeps a navigation's own query params and state beside the member", () => {
    setScopedMember("web");
    navigate("/graph?seed=a%20b", { q: "term" });
    expect(window.location.search).toBe("?seed=a%20b&repo=web");
    expect(window.history.state).toEqual({ q: "term" });
  });

  it("carries the member across the /overview redirect too", () => {
    // `redirect` replaces the entry; dropping the member here would re-open the
    // default one on every arrival at the retired bookmark.
    setScopedMember("web");
    redirect("/");
    expect(window.location.pathname + window.location.search).toBe("/?repo=web");
  });

  it("writes NOTHING when unscoped — single-root URLs stay byte-for-byte (ADR-52)", () => {
    const pushSpy = vi.spyOn(window.history, "pushState");
    navigate("/graph?seed=a%20b#node-3");
    // Byte-for-byte the pre-workspace URL: not re-encoded, not re-ordered, and with
    // no `?repo=` appended.
    expect(pushSpy).toHaveBeenCalledWith({}, "", "/graph?seed=a%20b#node-3");
    redirect("/health");
    expect(window.location.pathname + window.location.search).toBe("/health");
  });
});

describe("replaceUrl (S-426)", () => {
  it("replaces the entry in place, keeping history.state and firing no popstate", () => {
    window.history.replaceState({ q: "term" }, "", "/wiki/page/x");
    const pushSpy = vi.spyOn(window.history, "pushState");
    const dispatchSpy = vi.spyOn(window, "dispatchEvent");

    replaceUrl("/wiki/page/x?repo=web");

    expect(window.location.pathname + window.location.search).toBe("/wiki/page/x?repo=web");
    // No new back-stack entry: a member switch on the view you are looking at must
    // not cost a press of Back.
    expect(pushSpy).not.toHaveBeenCalled();
    // The entry's own ephemeral payload belongs to the entry, not to the member.
    expect(window.history.state).toEqual({ q: "term" });
    // And no popstate: the caller already committed this state, so re-notifying the
    // SPA would have it re-derive an answer it has.
    expect(dispatchSpy).not.toHaveBeenCalled();
  });
});

describe("currentUrl (S-426)", () => {
  it("is the whole in-SPA URL — path, query and fragment", () => {
    window.history.replaceState(null, "", "/graph?seed=x#node-3");
    expect(currentUrl()).toBe("/graph?seed=x#node-3");
  });
});
