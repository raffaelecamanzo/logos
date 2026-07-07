import { cleanup, render, renderHook, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

afterEach(cleanup);

import { ApiError } from "./client.ts";
import { AsyncResource, useApiResource } from "./hooks.tsx";

describe("useApiResource", () => {
  it("transitions loading → ready and exposes the data", async () => {
    const { result } = renderHook(() => useApiResource(() => Promise.resolve(42), []));
    expect(result.current.status).toBe("loading");
    await waitFor(() => expect(result.current.status).toBe("ready"));
    expect(result.current.data).toBe(42);
  });

  it("transitions loading → error and keeps the failure", async () => {
    const err = new ApiError("/api/v1/graph", 500);
    const { result } = renderHook(() => useApiResource(() => Promise.reject(err), []));
    await waitFor(() => expect(result.current.status).toBe("error"));
    expect(result.current.error).toBe(err);
  });
});

describe("AsyncResource", () => {
  const ready = <T,>(data: T) => ({ status: "ready" as const, data, error: undefined, reload: () => {} });

  it("renders a busy indicator while loading", () => {
    render(
      <AsyncResource resource={{ status: "loading", data: undefined, error: undefined, reload: () => {} }}>
        {() => <div>body</div>}
      </AsyncResource>,
    );
    expect(screen.getByRole("status")).toBeInTheDocument();
    expect(screen.queryByText("body")).not.toBeInTheDocument();
  });

  it("renders an honest error panel showing the failed path + status", () => {
    render(
      <AsyncResource
        resource={{
          status: "error",
          data: undefined,
          error: new ApiError("/api/v1/impact", 503),
          reload: () => {},
        }}
      >
        {() => <div>body</div>}
      </AsyncResource>,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("/api/v1/impact");
    expect(screen.getByRole("alert")).toHaveTextContent("503");
  });

  it("renders the empty slot when the view's isEmpty predicate holds", () => {
    render(
      <AsyncResource resource={ready<number[]>([])} isEmpty={(d) => d.length === 0} empty={<p>nothing</p>}>
        {() => <div>body</div>}
      </AsyncResource>,
    );
    expect(screen.getByText("nothing")).toBeInTheDocument();
  });

  it("renders the success body for non-empty ready data", () => {
    render(
      <AsyncResource resource={ready([1, 2])} isEmpty={(d) => d.length === 0}>
        {(d) => <div>count {d.length}</div>}
      </AsyncResource>,
    );
    expect(screen.getByText("count 2")).toBeInTheDocument();
  });
});

describe("stale-result drop", () => {
  it("ignores a superseded fetch so a slow earlier response cannot clobber a newer one", async () => {
    let resolveSlow: (v: string) => void = () => {};
    const slow = new Promise<string>((r) => (resolveSlow = r));
    const fetchers = [() => slow, () => Promise.resolve("new")];
    const { result, rerender } = renderHook(({ key }) => useApiResource(fetchers[key], [key]), {
      initialProps: { key: 0 },
    });
    rerender({ key: 1 }); // supersede the slow fetch before it settles
    await waitFor(() => expect(result.current.data).toBe("new"));
    resolveSlow("old"); // the superseded result must be dropped
    await Promise.resolve();
    expect(result.current.data).toBe("new");
  });
});
