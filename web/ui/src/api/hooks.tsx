/*
 * The SHARED `/api/v1` query hooks (S-186, CR-049, FR-UI-21) — the data-access
 * layer's React half, reused by every migrated view (S-187–S-189).
 *
 * `useApiResource` runs a same-origin fetch and exposes an honest four-state
 * machine — `loading | error | ready` (emptiness is a view-specific predicate the
 * consumer applies) — plus a `reload`. `AsyncResource` maps those states onto the
 * design-system honesty primitives (`LoadingState` / `ErrorPanel` / `EmptyState`,
 * S-193) so no view re-implements them and a failed read renders an honest panel,
 * never a fabricated figure (NFR-RA-05, NFR-CC-04). A read mutates no store
 * (ADR-28) — the hook only ever GETs.
 */

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";

import { EmptyState, ErrorPanel, LoadingState } from "../components/index.ts";
import { ApiError } from "./client.ts";

/** The lifecycle phase of an in-flight or settled fetch. */
export type ResourceStatus = "loading" | "error" | "ready";

/** The observable result of {@link useApiResource}. */
export interface ApiResource<T> {
  status: ResourceStatus;
  /** The fetched value once `status === "ready"`, else `undefined`. */
  data: T | undefined;
  /** The failure once `status === "error"`, else `undefined`. */
  error: Error | undefined;
  /** Re-run the fetch (e.g. a "retry" affordance, or a refresh). */
  reload: () => void;
}

/**
 * Fetch `fetcher()` and track it as a {@link ApiResource}. The fetch re-runs when
 * any `deps` entry changes (same contract as `useEffect`'s dependency array) and
 * on an explicit `reload()`. A result that arrives after the component unmounted,
 * or after a newer fetch superseded it, is dropped — so a fast filter change can
 * never let a stale response clobber a newer one (the legacy canvas's generation
 * guard, in hook form).
 */
export function useApiResource<T>(fetcher: () => Promise<T>, deps: readonly unknown[]): ApiResource<T> {
  const [state, setState] = useState<{ status: ResourceStatus; data?: T; error?: Error }>({
    status: "loading",
  });
  // A monotonic generation: only the latest fetch is allowed to commit its result.
  const generation = useRef(0);
  const [reloadTick, setReloadTick] = useState(0);

  // `fetcher` is intentionally not in the dependency array — callers pass an
  // inline closure that changes every render; `deps` is the real cache key (the
  // values the closure closes over). This mirrors useEffect's explicit-deps model.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const run = useCallback(fetcher, deps);

  useEffect(() => {
    const gen = ++generation.current;
    setState((prev) => (prev.status === "ready" ? prev : { status: "loading" }));
    run()
      .then((data) => {
        if (gen === generation.current) setState({ status: "ready", data });
      })
      .catch((err: unknown) => {
        if (gen === generation.current) {
          setState({ status: "error", error: err instanceof Error ? err : new Error(String(err)) });
        }
      });
    return () => {
      // Bump the generation so an in-flight result from this effect is dropped.
      generation.current++;
    };
  }, [run, reloadTick]);

  const reload = useCallback(() => setReloadTick((t) => t + 1), []);
  return { status: state.status, data: state.data, error: state.error, reload };
}

/** Props for {@link AsyncResource}. */
export interface AsyncResourceProps<T> {
  /** The resource to render. */
  resource: ApiResource<T>;
  /** Accessible + visible label while loading (e.g. "Loading the graph…"). */
  loadingLabel?: string;
  /** Optional predicate: when it returns true for the ready data, the empty slot renders. */
  isEmpty?: (data: T) => boolean;
  /** What to show when `isEmpty` holds (a design-system `EmptyState`, typically). */
  empty?: ReactNode;
  /** The success renderer — receives the non-empty data. */
  children: (data: T) => ReactNode;
}

/**
 * Render a {@link ApiResource} through the design-system honesty primitives:
 * a busy indicator while loading, an honest error panel on failure (the façade's
 * `ApiError` is shown verbatim — never papered over), the `empty` slot when the
 * view-specific `isEmpty` predicate holds, else the success `children`.
 */
export function AsyncResource<T>({
  resource,
  loadingLabel,
  isEmpty,
  empty,
  children,
}: AsyncResourceProps<T>): ReactNode {
  if (resource.status === "loading") return <LoadingState label={loadingLabel} />;
  if (resource.status === "error") {
    return <ErrorPanel>{describeError(resource.error)}</ErrorPanel>;
  }
  const data = resource.data as T;
  if (isEmpty?.(data)) return empty ?? <EmptyState message="Nothing to show yet." />;
  return children(data);
}

/** A human, non-fabricated description of a failed read (NFR-RA-05). */
function describeError(error: Error | undefined): string {
  if (error instanceof ApiError) {
    return `The request to ${error.path} failed (HTTP ${error.status}).`;
  }
  return error?.message ?? "The request could not be completed.";
}
