/*
 * The shell header (S-185, re-skinned onto the design system in S-193). Carries
 * the brand, the graph-state readout, and the theme toggle.
 *
 * The readout (S-315, CR-097, FR-UI-34) reports the state of the graph the user is
 * looking at — `rev <graph_revision> · <node_count> nodes · <edge_count> edges`,
 * read verbatim from the FR-NV-07 status read-model over `GET /api/v1/status` and
 * computed nowhere (NFR-RA-05, ADR-01). It replaces the green "Read-model
 * connected" badge, which was decided by a loopback request to the very process
 * answering it — so it could only ever display success — and which paid ~1 MB per
 * navigation (the FR-UI-04 Health bundle) to set one boolean.
 *
 * Degradation stays honest (NFR-RA-05, NFR-CC-04): a read fault renders the
 * retained red "API unavailable" badge IN PLACE OF the figures — never a zero,
 * never a blank, never a previously-read figure presented as current — and an
 * un-indexed project states that rather than reporting `0 nodes · 0 edges`, which
 * would read as a measurement. Colour is never the only signal: every state carries
 * text.
 *
 * Refresh is navigation-driven and NOTHING else (FR-UI-34): the read re-fires when
 * the client route changes, and when the workspace member or mode changes, so the
 * figures always describe the member being presented (FR-UI-29). There is no
 * interval, no visibility handler, and no other trigger — a background timer would
 * emit continuous telemetry and load for a figure nobody observes between actions
 * (BR-42, FR-OB-09).
 */

import { useEffect, useState } from "react";

import { Badge, ThemeToggle } from "../components/index.ts";
import { fetchStatus } from "../api/client.ts";
import type { StatusInfo } from "../api/types.ts";
import { navigate, usePathname } from "../router.tsx";
import { useWorkspace } from "../workspace/WorkspaceContext.tsx";
import { WorkspaceFault } from "./WorkspaceFault.tsx";
import styles from "./Header.module.css";

/** The header's read, as an honest three-state machine. `ready` carries the
 *  read-model itself, so no state can render a figure the server did not send. */
type Readout =
  | { kind: "loading" }
  | { kind: "ready"; status: StatusInfo }
  | { kind: "error" };

/** The Dashboard route the brand lockup links to (root since S-194). */
const DASHBOARD_PATH = "/";

/** The authored brand mark, inlined from `web/assets/favicon.svg` — a merlin square
 *  with the red peak. Inline SVG keeps it self-contained (no external origin, no
 *  vendored asset), so the self-only CSP and offline posture are unaffected. */
function BrandMark() {
  return (
    <svg
      className={styles.brandLogo}
      viewBox="0 0 32 32"
      width="28"
      height="28"
      aria-hidden="true"
      focusable="false"
    >
      <rect width="32" height="32" rx="6" fill="#3d3935" />
      <path d="M16 6.5 L25.5 25.5 H19.6 L16 18 L12.4 25.5 H6.5 Z" fill="#da291c" />
    </svg>
  );
}

/**
 * The readout line for an indexed graph — a presentation projection of three
 * read-model fields and nothing else. The digits are grouped for legibility
 * (`12345` → `12,345`), matching the Dashboard's `fmtInt` and the Statistics tab's
 * `num` so the three surfaces read alike; it is built as ONE string so the figures
 * are a single text node rather than a run of fragments.
 */
function readoutLine(status: StatusInfo): string {
  const group = (n: number) => n.toLocaleString("en-US");
  return `rev ${group(status.graph_revision)} · ${group(status.node_count)} nodes · ${group(
    status.edge_count,
  )} edges`;
}

export function Header() {
  const [readout, setReadout] = useState<Readout>({ kind: "loading" });
  // Three dependencies, and each is load-bearing. The header sits outside the
  // member-keyed view subtree, so the member (`cacheKey`) and the workspace `mode`
  // are explicit: the figures must describe the member the shell is CURRENTLY
  // presenting, or one member's counts would sit inches from another's name (S-250,
  // FR-UI-29). The `pathname` is what makes the refresh navigation-driven — the
  // router already tracks it, so joining it here needs no new mechanism and adds no
  // timer (FR-UI-34).
  const { cacheKey, mode, unknownMember } = useWorkspace();
  const pathname = usePathname();

  useEffect(() => {
    // Two states in which there is no member to report figures for, and the badge
    // must show none rather than the wrong ones:
    //   - the probe has not settled, so we do not know the member yet and a request
    //     now would go out unscoped and have to be re-issued anyway;
    //   - the URL names a member this workspace does not have (S-426), so an
    //     unscoped read would answer from the DEFAULT member and put its counts
    //     inches from the requested member's name (NFR-RA-05) — the same
    //     substitution the content slot refuses by rendering no view at all.
    // The readout is reset rather than merely left alone: arriving here from a valid
    // member (a back/forward into an unknown one) would otherwise keep that member's
    // figures on screen.
    setReadout({ kind: "loading" });
    if (mode === "loading" || unknownMember !== null) return;
    let alive = true;
    // Through the typed client, so the read carries the active `?repo=` scope. In
    // single-root mode no param is appended and the request is byte-for-byte the
    // shape every other read has.
    fetchStatus()
      .then((status) => {
        if (alive) setReadout({ kind: "ready", status });
      })
      .catch(() => {
        // The figures are DROPPED, not retained: a stale count presented as current
        // is the failure mode this badge exists to avoid (NFR-RA-05).
        if (alive) setReadout({ kind: "error" });
      });
    return () => {
      alive = false;
    };
  }, [cacheKey, mode, unknownMember, pathname]);

  return (
    <header className={styles.header}>
      {/* The brand lockup is a link home to the Dashboard (client-side nav, no
          full reload). */}
      <a
        className={styles.brand}
        href={DASHBOARD_PATH}
        aria-label="Logos — go to Dashboard"
        onClick={(e) => {
          e.preventDefault();
          navigate(DASHBOARD_PATH);
        }}
      >
        <BrandMark />
        <span className={styles.brandMark}>Logos</span>
      </a>
      <div className={styles.spacer} />
      {/* A FAULTED workspace probe only — in a single-root serve, and in a healthy
          workspace serve alike, this renders nothing and the header is byte-for-byte
          unchanged (FR-UI-29, NFR-RA-05). The member SELECTOR is not here: S-425
          moved it into the sidebar's Service-section header, inside the boundary it
          governs (FR-UI-35, ADR-66). */}
      <WorkspaceFault />
      <span className={styles.status}>
        {readout.kind === "loading" && <Badge tone="muted">Connecting…</Badge>}
        {readout.kind === "error" && (
          // The fault is the ONE state that carries a live region. The figures change
          // on every navigation, and announcing them each time would make the shell
          // the noisiest thing on the page; an unavailable read must be announced
          // (FR-UI-34).
          <span role="status">
            <Badge tone="red">API unavailable</Badge>
          </span>
        )}
        {readout.kind === "ready" &&
          (readout.status.indexed ? (
            <span className={styles.readout}>{readoutLine(readout.status)}</span>
          ) : (
            // An un-indexed project has no figures to report. Saying so is honest;
            // `0 nodes · 0 edges` would read as a measurement (NFR-CC-04).
            <Badge tone="muted">Not indexed</Badge>
          ))}
      </span>
      <ThemeToggle />
    </header>
  );
}
