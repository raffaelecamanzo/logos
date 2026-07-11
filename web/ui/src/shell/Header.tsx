/*
 * The shell header (S-185, re-skinned onto the design system in S-193). Carries
 * the brand, a live read-model connectivity indicator, and the theme toggle. On
 * mount it fetches `/api/v1/health` same-origin, proving the SPA talks to the JSON
 * seam and contacts no external origin (AC: "the shell fetches its data from
 * /api/v1"). The connectivity state renders as a tone-carrying Badge (colour is
 * never the only signal): green when connected, red on a read fault — an honest
 * "unavailable" state, never a fabricated figure (NFR-RA-05, NFR-CC-04).
 */

import { useEffect, useState } from "react";

import { Badge, ThemeToggle } from "../components/index.ts";
import { apiGet } from "../intent.ts";
import { apiUrl } from "../api/client.ts";
import { navigate } from "../router.tsx";
import { useWorkspace } from "../workspace/WorkspaceContext.tsx";
import { MemberSelector } from "./MemberSelector.tsx";
import styles from "./Header.module.css";

type ApiState = "loading" | "ok" | "error";

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

export function Header() {
  const [state, setState] = useState<ApiState>("loading");
  // The header sits outside the member-keyed view subtree, so the member is an
  // explicit dependency: the badge must report the member the shell is CURRENTLY
  // presenting. Otherwise selecting a member whose engine cannot start would leave a
  // green "connected" badge sitting inches from that member's name (S-250).
  const { cacheKey, mode } = useWorkspace();

  useEffect(() => {
    // Until the workspace probe settles we do not know the member, so a request now
    // would go out unscoped and have to be re-issued anyway. Stay honestly "Connecting…".
    if (mode === "loading") return;
    let alive = true;
    setState("loading");
    // Through `apiUrl`, so the probe carries the active `?repo=` scope. In single-root
    // mode no param is appended and the request is byte-for-byte the one it always was.
    apiGet(apiUrl("health"))
      .then(() => {
        if (alive) setState("ok");
      })
      .catch(() => {
        if (alive) setState("error");
      });
    return () => {
      alive = false;
    };
  }, [cacheKey, mode]);

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
        <span className={styles.brandSub}>code intelligence</span>
      </a>
      <div className={styles.spacer} />
      {/* Workspace mode only — in a single-root serve this renders nothing and the
          header is byte-for-byte unchanged (FR-UI-29). */}
      <MemberSelector />
      <span className={styles.status} role="status">
        {state === "loading" && <Badge tone="muted">Connecting…</Badge>}
        {state === "ok" && <Badge tone="green">Read-model connected</Badge>}
        {state === "error" && <Badge tone="red">API unavailable</Badge>}
      </span>
      <ThemeToggle />
    </header>
  );
}
