import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { fetchHealth } from "../api/client.ts";
import { useWorkspace, WorkspaceProvider } from "./WorkspaceContext.tsx";
import { scopedMember, setScopedMember } from "./scope.ts";
import { stubApi } from "./testFixtures.ts";

/** The member the switch-probe below selects — a real roster member, and NOT the
 *  one the default opens on, so a switch that did nothing would be visible. */
const OTHER_MEMBER = "web";

/** A probe of the context — every field the shell reads. */
function Probe() {
  const { mode, workspace, members, member, cacheKey, error, unknownMember, selectMember } =
    useWorkspace();
  return (
    <div>
      <span data-testid="mode">{mode}</span>
      <span data-testid="workspace">{workspace ?? "—"}</span>
      <span data-testid="members">{members.join(",")}</span>
      <span data-testid="member">{member ?? "—"}</span>
      <span data-testid="key">{cacheKey}</span>
      <span data-testid="error">{error ? "error" : "—"}</span>
      <span data-testid="unknown">{unknownMember ?? "—"}</span>
      {/* The shell's selector reduced to its one contract: `selectMember(name)`.
          The real control is asserted in `MemberSelector.test.tsx`. */}
      <button data-testid="select" onClick={() => selectMember(OTHER_MEMBER)}>
        select
      </button>
      {/* The same selection, padded — one rule, applied wherever a name enters. */}
      <button data-testid="select-padded" onClick={() => selectMember(`  ${OTHER_MEMBER}  `)}>
        select padded
      </button>
    </div>
  );
}

function mount() {
  return render(
    <WorkspaceProvider>
      <Probe />
    </WorkspaceProvider>,
  );
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  setScopedMember(null);
  // The URL is shared state across specs now that the provider both reads and
  // writes it. A spec that opened on `?repo=` must not leak it into the next.
  window.history.replaceState(null, "", "/");
});

/** Open the SPA on `url` — what a bookmark, a shared link or a refresh supplies. */
function openAt(url: string) {
  window.history.replaceState(null, "", url);
}

/** Every `/api/v1` read that carried a member scope, in order. */
const scopedCalls = (calls: string[]) => calls.filter((u) => u.includes("repo="));

describe("WorkspaceProvider mode discovery (S-250, FR-UI-29, FR-WS-06)", () => {
  it("reads the roster's honest 404 as single-root — and scopes NOTHING", async () => {
    stubApi({ probeStatus: 404 });
    mount();
    await waitFor(() => expect(screen.getByTestId("mode")).toHaveTextContent("single"));
    expect(screen.getByTestId("member")).toHaveTextContent("—");
    expect(screen.getByTestId("error")).toHaveTextContent("—");
    // The decisive single-root guarantee: no member scope, so no request ever carries
    // `?repo=` and every URL stays byte-for-byte the pre-workspace one.
    expect(scopedMember()).toBeNull();
    // …and the cache key never changes, so nothing remounts or re-fetches.
    expect(screen.getByTestId("key")).toHaveTextContent("single");
  });

  it("reads a 200 as workspace mode and rosters the members", async () => {
    stubApi();
    mount();
    await waitFor(() => expect(screen.getByTestId("mode")).toHaveTextContent("workspace"));
    expect(screen.getByTestId("workspace")).toHaveTextContent("shop");
    expect(screen.getByTestId("members")).toHaveTextContent("api,web");
  });

  it("probes the ENGINE-FREE roster, never the all-member status fan-out (NFR-PE-10)", async () => {
    const calls = stubApi();
    mount();
    await waitFor(() => expect(screen.getByTestId("mode")).toHaveTextContent("workspace"));
    // The shell probes on every page load. `workspace/status` fans out over every
    // member — probing it here would construct and watch all N engines on first paint.
    expect(calls().some((u) => u.startsWith("/api/v1/workspace/roster"))).toBe(true);
    expect(calls().some((u) => u.startsWith("/api/v1/workspace/status"))).toBe(false);
  });

  it("opens on the manifest DEFAULT member, not merely the first in the roster", async () => {
    // The roster's default is `api`; if the SPA opened on `members[0]` blindly it would
    // agree here by luck. Put the default second so the two genuinely differ.
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) =>
        Promise.resolve({
          ok: true,
          status: 200,
          json: () =>
            Promise.resolve(
              url.startsWith("/api/v1/workspace/roster")
                ? { workspace: "shop", default: "web", members: ["api", "web"] }
                : {},
            ),
        } as Response),
      ),
    );
    mount();
    await waitFor(() => expect(screen.getByTestId("member")).toHaveTextContent("web"));
    // Unscoped requests answer from the default member, so opening anywhere else would
    // show a different member than the CLI and the plain API do.
    expect(scopedMember()).toBe("web");
  });

  it("namespaces the cache key so a member literally named `single` cannot collide", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) =>
        Promise.resolve({
          ok: true,
          status: 200,
          json: () =>
            Promise.resolve(
              url.startsWith("/api/v1/workspace/roster")
                ? { workspace: "shop", default: "single", members: ["single"] }
                : {},
            ),
        } as Response),
      ),
    );
    mount();
    await waitFor(() => expect(screen.getByTestId("member")).toHaveTextContent("single"));
    // If the key were the bare member name it would equal the pre-probe sentinel, the
    // view would never remount on the mode flip, and it would keep the unscoped data.
    expect(screen.getByTestId("key")).toHaveTextContent("member:single");
  });

  it("scopes the transport so every existing view's read carries ?repo=<member>", async () => {
    const calls = stubApi();
    mount();
    await waitFor(() => expect(screen.getByTestId("mode")).toHaveTextContent("workspace"));

    await fetchHealth();
    expect(calls().at(-1)).toBe("/api/v1/health?repo=api");
  });

  it("surfaces a genuine probe fault instead of passing it off as a plain repo", async () => {
    stubApi({ probeStatus: 500 });
    mount();
    await waitFor(() => expect(screen.getByTestId("error")).toHaveTextContent("error"));
    expect(scopedMember()).toBeNull();
  });
});

// ── S-426 / FR-UI-35 / NFR-RA-05: the member is deep-linkable, never substituted ──

describe("the URL names the member (S-426, FR-UI-35)", () => {
  it("opens on the member the URL names, not on the manifest default", async () => {
    // This spec pins WHICH member the provider settles on. It does NOT pin the
    // absence of an unscoped pre-pass: this probe mounts no view, so a provider that
    // resolved the default and then corrected itself would settle here identically.
    // That property is asserted where a real view actually reads — see
    // `App.workspace.test.tsx`, "issues exactly ONE read".
    openAt("/?repo=web");
    const calls = stubApi();
    mount();
    await waitFor(() => expect(screen.getByTestId("member")).toHaveTextContent("web"));

    await fetchHealth();
    // `api` is the roster's DEFAULT, so if any read had gone out before the URL was
    // consulted it would be sitting in this list.
    expect(scopedCalls(calls())).toEqual(["/api/v1/health?repo=web"]);
    expect(scopedMember()).toBe("web");
  });

  it("falls back to the manifest default when the URL names no member", async () => {
    const calls = stubApi();
    mount();
    await waitFor(() => expect(screen.getByTestId("member")).toHaveTextContent("api"));
    await fetchHealth();
    expect(scopedCalls(calls())).toEqual(["/api/v1/health?repo=api"]);
  });

  it.each([
    ["/?repo=", "blank"],
    ["/?repo=%20", "whitespace-only"],
    ["/?repo=+", "a plus-encoded space"],
  ])("treats %s (%s) as UNSCOPED, never a member named empty", async (url) => {
    // The rule `member.rs` applies server-side, applied identically here — the row
    // set both spellings are pinned against lives in `repo-param-cases.txt`.
    openAt(url);
    stubApi();
    mount();
    // Unscoped ⇒ the manifest default, and no refusal.
    await waitFor(() => expect(screen.getByTestId("member")).toHaveTextContent("api"));
    expect(screen.getByTestId("unknown")).toHaveTextContent("—");
  });

  it("writes the switch through history — replaceState, no new back-stack entry", async () => {
    const pushSpy = vi.spyOn(window.history, "pushState");
    stubApi();
    mount();
    await waitFor(() => expect(screen.getByTestId("member")).toHaveTextContent("api"));

    act(() => screen.getByTestId("select").click());

    expect(window.location.search).toBe("?repo=web");
    // A same-view member switch must not cost a press of Back.
    expect(pushSpy).not.toHaveBeenCalled();
  });

  it("normalises a selected name everywhere, not only in the transport", async () => {
    // The transport scope, the React state (and so the cache key) and the URL are
    // three sites one name reaches. Normalising only the first would scope reads to
    // `web` while the URL said `%20%20web%20%20` and the view subtree was keyed on a
    // third spelling — one member wearing three names.
    stubApi();
    mount();
    await waitFor(() => expect(screen.getByTestId("member")).toHaveTextContent("api"));

    act(() => screen.getByTestId("select-padded").click());

    expect(screen.getByTestId("member")).toHaveTextContent("web");
    expect(screen.getByTestId("key")).toHaveTextContent("member:web");
    expect(scopedMember()).toBe("web");
    expect(window.location.search).toBe("?repo=web");
  });

  it("keeps the rest of the URL — path, other params and fragment — across a switch", async () => {
    openAt("/graph?seed=a%20b&repo=api#node-3");
    stubApi();
    mount();
    await waitFor(() => expect(screen.getByTestId("member")).toHaveTextContent("api"));

    act(() => screen.getByTestId("select").click());

    expect(window.location.pathname).toBe("/graph");
    expect(window.location.search).toBe("?seed=a%20b&repo=web");
    expect(window.location.hash).toBe("#node-3");
  });

  it("restores the member a back/forward entry names", async () => {
    openAt("/?repo=web");
    stubApi();
    mount();
    await waitFor(() => expect(screen.getByTestId("member")).toHaveTextContent("web"));

    // What the browser does on Back: the URL changes, then popstate fires.
    act(() => {
      window.history.replaceState(null, "", "/?repo=api");
      window.dispatchEvent(new PopStateEvent("popstate"));
    });

    await waitFor(() => expect(screen.getByTestId("member")).toHaveTextContent("api"));
    expect(scopedMember()).toBe("api");
  });

  it("registers exactly one popstate listener and removes it on unmount", () => {
    // Neither half is visible in a behaviour assertion: a leaked listener keeps
    // resolving members for an unmounted provider, and a duplicate one resolves
    // twice per back/forward. Both are silent, so they are counted.
    const add = vi.spyOn(window, "addEventListener");
    const remove = vi.spyOn(window, "removeEventListener");
    stubApi();
    const { unmount } = mount();
    const pops = () => add.mock.calls.filter(([type]) => type === "popstate").length;
    const unpops = () => remove.mock.calls.filter(([type]) => type === "popstate").length;

    return waitFor(() => expect(pops()).toBe(1)).then(() => {
      expect(unpops()).toBe(0);
      unmount();
      expect(unpops()).toBe(1);
      expect(pops()).toBe(1);
    });
  });
});

describe("an unknown member is refused, not substituted (S-426, NFR-RA-05)", () => {
  it("names the members the workspace DOES have, and scopes nothing", async () => {
    openAt("/?repo=ghost");
    const calls = stubApi();
    mount();
    await waitFor(() => expect(screen.getByTestId("unknown")).toHaveTextContent("ghost"));

    // No member is selected and NOTHING is scoped: a read issued now would be
    // unscoped, which is why `App.tsx` mounts no view at all in this state.
    expect(screen.getByTestId("member")).toHaveTextContent("—");
    expect(scopedMember()).toBeNull();
    // The roster is still in hand — that is what lets the refusal name the members.
    expect(screen.getByTestId("members")).toHaveTextContent("api,web");
    // And no member-scoped read ever went out under the requested name.
    expect(scopedCalls(calls())).toEqual([]);
  });

  it("is decided against the roster alone — no request, no engine started", async () => {
    openAt("/?repo=ghost");
    const calls = stubApi();
    mount();
    await waitFor(() => expect(screen.getByTestId("unknown")).toHaveTextContent("ghost"));
    // The manifest roster the shell already probed answers this; asking the server
    // would cost a round-trip to be told what is already on the client.
    expect(calls()).toEqual(["/api/v1/workspace/roster"]);
  });

  it("clears on selecting a real member, which re-scopes and rewrites the URL", async () => {
    openAt("/?repo=ghost");
    stubApi();
    mount();
    await waitFor(() => expect(screen.getByTestId("unknown")).toHaveTextContent("ghost"));

    act(() => screen.getByTestId("select").click());

    await waitFor(() => expect(screen.getByTestId("unknown")).toHaveTextContent("—"));
    expect(screen.getByTestId("member")).toHaveTextContent("web");
    expect(scopedMember()).toBe("web");
    expect(window.location.search).toBe("?repo=web");
  });

  it("does NOT refuse a member that is in the roster — that failure is the read's 500", async () => {
    // The distinction `member.rs` draws and the client must not re-merge: `web` IS a
    // member, so it scopes normally and a broken engine surfaces as the read's own
    // 500 in the view. Refusing here would send the user hunting for a typo.
    openAt("/?repo=web");
    stubApi();
    mount();
    await waitFor(() => expect(screen.getByTestId("member")).toHaveTextContent("web"));
    expect(screen.getByTestId("unknown")).toHaveTextContent("—");
  });
});

describe("single-root stays inert (S-426, ADR-52)", () => {
  it("ignores a HAND-TYPED ?repo= entirely — no scope, no request param, no rewrite", async () => {
    openAt("/?repo=web");
    const calls = stubApi({ probeStatus: 404 });
    mount();
    await waitFor(() => expect(screen.getByTestId("mode")).toHaveTextContent("single"));

    await fetchHealth();
    // Neither honoured…
    expect(scopedMember()).toBeNull();
    expect(screen.getByTestId("member")).toHaveTextContent("—");
    expect(screen.getByTestId("unknown")).toHaveTextContent("—");
    expect(calls().at(-1)).toBe("/api/v1/health");
    // …nor stripped: it stays exactly where it was typed, inert, like every other
    // unrecognised query param always has.
    expect(window.location.search).toBe("?repo=web");
  });
});
