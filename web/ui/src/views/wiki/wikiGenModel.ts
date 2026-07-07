/*
 * Wiki-generation view model (S-178, CR-047, FR-WK-18, FR-UI-19, NFR-SE-07) — the
 * PURE half of the Wiki tab's generation trigger: the SSE wire types, the per-run
 * event reducer (run start / per-page lifecycle / honest halt / honest error /
 * configure-first / busy), and the consent + endpoint-disclosure helpers.
 *
 * It holds NO React and NO network: the wiki-agent's `WikiProgress` contract is
 * consumed verbatim (`wiki-agent/src/agent.rs`, serde-tagged on `event`,
 * `rename_all = "kebab-case"`). Keeping the reducer pure makes every branch (incl.
 * `halted`/`error`/`busy`) deterministically testable without a DOM or a fetch, and
 * lets the component (`WikiView.tsx`) stay a thin renderer over it.
 *
 * The masked API key is NEVER referenced here — the consent banner discloses only
 * the provider, the effective wiki model, and the endpoint host (NFR-SE-07).
 */

import type { ConfigReadModel } from "../../api/types.ts";
import type { SseFrame } from "../../api/sse.ts";

// ── SSE wire types (mirror wiki-agent's `WikiProgress`, serde-tagged on `event`,
//    `rename_all = "kebab-case"`). Internal to the bundled frontend, not a public
//    API. The non-progress frames (`configure-first`/`error`/`busy`) carry a
//    plain-text `data` payload, not JSON. ──

/** A streamed per-page generation transition (mirrors `WikiProgress`). */
export type WikiProgress =
  | { event: "started"; total: number; synthesis_timeout_secs: number }
  | { event: "page-started"; slug: string; title: string; index: number; total: number }
  | { event: "page-written"; slug: string; anchor_count: number; replaced: boolean }
  | { event: "page-failed"; slug: string; error: string }
  | { event: "halted"; reason: string }
  | { event: "completed"; pages_written: number; pages_failed: number };

// ── Per-run state machine ──────────────────────────────────────────────────────

/** The lifecycle phase of the Wiki-tab generation run. */
export type WikiGenPhase =
  /** No run triggered yet (or none needed). */
  | "idle"
  /** A run is streaming per-page refreshes. */
  | "running"
  /** The run finished (completed or honestly halted). */
  | "done"
  /** No provider is configured — the honest configure-first state ([FR-UI-18]). */
  | "configure-first"
  /** A run was already in flight, so this open started none ([FR-WK-18]). */
  | "busy"
  /** The run faulted (setup/provider/infrastructure) — surfaced honestly. */
  | "error";

/** One failed page in a run. */
export interface WikiPageFailure {
  slug: string;
  error: string;
}

/** The accumulated render state of one generation run, folded from its SSE frames. */
export interface WikiGenState {
  phase: WikiGenPhase;
  /** The number of queued pages the run will attempt (from `started`). */
  total: number;
  /** The slugs written this run, in order (each `page-written`). */
  written: string[];
  /** The pages whose write was rejected this run (each `page-failed`). */
  failed: WikiPageFailure[];
  /** The slug currently being synthesized, or `null` between pages. */
  current: string | null;
  /** The honest halt reason when the run stopped early (budget/provider), else `null`. */
  halted: string | null;
  /** The configure-first / error message to show, else `null`. */
  message: string | null;
  /** The configured per-page synthesis liveness timeout (`Started`'s
   *  `synthesis_timeout_secs`, [CR-059], [S-239]), or `null` before a `started` frame
   *  arrives. Surfaced so a long-running page reads as a liveness guard counting
   *  down, not an unexplained stall ([FR-UI-24]). */
  synthesisTimeoutSecs: number | null;
}

/** A fresh, pre-run state. */
export function initialWikiGenState(): WikiGenState {
  return {
    phase: "idle",
    total: 0,
    written: [],
    failed: [],
    current: null,
    halted: null,
    message: null,
    synthesisTimeoutSecs: null,
  };
}

/**
 * Fold one SSE frame into a run's state, returning a NEW state (immutable — the
 * component drives React re-renders off the reference). Honest by construction
 * ([NFR-CC-04]): `configure-first`/`error`/`busy` carry a plain-text body verbatim,
 * `halted` records the named reason, and a malformed (non-plain-text) frame is
 * dropped rather than guessed at — never a fabricated page.
 */
export function applyWikiFrame(state: WikiGenState, frame: SseFrame): WikiGenState {
  // The non-progress frames carry a PLAIN-TEXT payload, not JSON (NFR-CC-04).
  if (frame.name === "configure-first") {
    return { ...state, phase: "configure-first", message: frame.data };
  }
  if (frame.name === "busy") {
    return { ...state, phase: "busy" };
  }
  if (frame.name === "error") {
    return { ...state, phase: "error", message: frame.data };
  }
  let data: Record<string, unknown>;
  try {
    data = JSON.parse(frame.data) as Record<string, unknown>;
  } catch {
    return state; // a malformed frame is dropped rather than guessed at
  }
  switch (frame.name) {
    case "started": {
      const timeout = Number(data.synthesis_timeout_secs);
      return {
        ...state,
        phase: "running",
        total: Number(data.total) || 0,
        synthesisTimeoutSecs: Number.isFinite(timeout) && timeout > 0 ? timeout : null,
      };
    }
    case "page-started": {
      const slug = typeof data.slug === "string" ? data.slug : null;
      return { ...state, phase: "running", current: slug };
    }
    case "page-written": {
      const slug = typeof data.slug === "string" ? data.slug : null;
      if (!slug) return state;
      return {
        ...state,
        phase: "running",
        written: state.written.includes(slug) ? state.written : [...state.written, slug],
        current: state.current === slug ? null : state.current,
      };
    }
    case "page-failed": {
      const slug = typeof data.slug === "string" ? data.slug : "";
      const error = typeof data.error === "string" ? data.error : "the page write was rejected";
      return {
        ...state,
        failed: [...state.failed, { slug, error }],
        current: state.current === slug ? null : state.current,
      };
    }
    case "halted":
      return { ...state, halted: typeof data.reason === "string" ? data.reason : "the run halted" };
    case "completed":
      // The terminal event: the run is done (its per-page effects already folded).
      return { ...state, phase: "done", current: null };
    default:
      return state;
  }
}

// ── Config-driven configure-first + endpoint disclosure ───────────────────────

/** The native Anthropic endpoint host disclosed for the `anthropic` provider
 *  (mirrors the chat surface's `ANTHROPIC_HOST`). */
export const ANTHROPIC_HOST = "api.anthropic.com";

/** Extract the host authority from a URL — the run between `://` and the next `/`,
 *  falling back to the trimmed input when it carries no scheme (so a misconfigured
 *  `base_url` shows honestly rather than blanked). Mirrors the chat surface's `hostOf`. */
export function hostOf(url: string): string {
  const afterScheme = url.includes("://") ? url.slice(url.indexOf("://") + 3) : url;
  const host = afterScheme.split("/")[0].trim();
  return host === "" ? url.trim() : host;
}

/** The effective wiki model ([FR-CF-07]): `[wiki].model` if set, else `[chat].model`;
 *  `null` when neither resolves (the configure-first state). Mirrors the server's
 *  `WikiConfig::resolve` fallback. */
export function effectiveWikiModel(config: ConfigReadModel): string | null {
  const parsed = config.config.parsed;
  const wikiModel = parsed.wiki?.model?.trim();
  if (wikiModel) return wikiModel;
  const chatModel = parsed.chat.model?.trim();
  return chatModel && chatModel !== "" ? chatModel : null;
}

/** Is wiki generation usable? An effective model AND a present key (mirrors the
 *  server's configure-first predicate — a model with no key is still configure-first). */
export function isWikiConfigured(config: ConfigReadModel): boolean {
  return effectiveWikiModel(config) !== null && config.chat_key.present;
}

/** The provider disclosure the first-use consent banner names (NFR-SE-07): the
 *  provider, the effective wiki model, and the endpoint host — never the key. */
export interface WikiDisclosure {
  provider: string;
  model: string;
  endpointHost: string;
}

/** Compose the consent disclosure from the config read-model (NFR-SE-07). */
export function wikiDisclosure(config: ConfigReadModel): WikiDisclosure {
  const chat = config.config.parsed.chat;
  const model = effectiveWikiModel(config) ?? "(no model)";
  const endpointHost = chat.provider === "anthropic" ? ANTHROPIC_HOST : hostOf(chat.base_url);
  return { provider: chat.provider, model, endpointHost };
}

// ── Consent gate (NFR-SE-07; mirrors the chat consent localStorage gate) ──────

/** The localStorage key remembering the first-use wiki-generation consent
 *  acknowledgement — distinct from the chat consent key so the two are independent. */
export const WIKI_CONSENT_KEY = "logos.wiki.consent";

/** Has the user acknowledged the first-use wiki-generation consent? Storage-blocked
 *  ⇒ re-ask each load (fail SAFE, not open). */
export function hasWikiConsent(): boolean {
  try {
    return window.localStorage.getItem(WIKI_CONSENT_KEY) === "1";
  } catch {
    return false;
  }
}

/** Remember the consent acknowledgement (best-effort; non-fatal if storage is blocked). */
export function rememberWikiConsent(): void {
  try {
    window.localStorage.setItem(WIKI_CONSENT_KEY, "1");
  } catch {
    /* non-fatal: consent holds for this page even if it cannot persist */
  }
}
