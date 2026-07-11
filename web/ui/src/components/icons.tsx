/*
 * Icon set (S-193, FR-UI-23; frontend-design §3, CR-042). Idiomatic Lucide-style
 * line glyphs as inline SVG — `currentColor`, decorative (`aria-hidden`), no
 * vendored asset and no external origin (CSP-safe). Each is a tiny stroked path;
 * the sidebar pairs one with every nav label.
 */

import type { SVGProps } from "react";

type IconProps = SVGProps<SVGSVGElement>;

function Svg({ children, ...rest }: IconProps & { children: React.ReactNode }) {
  return (
    <svg
      width="18"
      height="18"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      {...rest}
    >
      {children}
    </svg>
  );
}

export const IconDashboard = (p: IconProps) => (
  <Svg {...p}>
    <rect x="3" y="3" width="7" height="9" />
    <rect x="14" y="3" width="7" height="5" />
    <rect x="14" y="12" width="7" height="9" />
    <rect x="3" y="16" width="7" height="5" />
  </Svg>
);

export const IconHealth = (p: IconProps) => (
  <Svg {...p}>
    <path d="M3 12h4l2 5 4-12 2 7h6" />
  </Svg>
);

export const IconGraph = (p: IconProps) => (
  <Svg {...p}>
    <circle cx="6" cy="6" r="2.5" />
    <circle cx="18" cy="9" r="2.5" />
    <circle cx="9" cy="18" r="2.5" />
    <path d="M8 7.5 15.5 9M7.5 8 9 15.5" />
  </Svg>
);

export const IconChat = (p: IconProps) => (
  <Svg {...p}>
    <path d="M21 15a2 2 0 0 1-2 2H8l-4 4V5a2 2 0 0 1 2-2h13a2 2 0 0 1 2 2z" />
  </Svg>
);

export const IconWiki = (p: IconProps) => (
  <Svg {...p}>
    <path d="M4 4h11a3 3 0 0 1 3 3v13H7a3 3 0 0 1-3-3z" />
    <path d="M18 7h2v13H7" />
  </Svg>
);

export const IconArchitecture = (p: IconProps) => (
  <Svg {...p}>
    <rect x="3" y="3" width="6" height="6" />
    <rect x="15" y="15" width="6" height="6" />
    <path d="M9 6h6a3 3 0 0 1 3 3v6" />
  </Svg>
);

export const IconFiles = (p: IconProps) => (
  <Svg {...p}>
    <path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z" />
    <path d="M14 3v6h6" />
  </Svg>
);

export const IconGaps = (p: IconProps) => (
  <Svg {...p}>
    <path d="M12 9v4M12 17h.01" />
    <path d="M10.3 3.9 2.4 18a2 2 0 0 0 1.7 3h15.8a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z" />
  </Svg>
);

export const IconCoverage = (p: IconProps) => (
  <Svg {...p}>
    <path d="M22 11.1V12a10 10 0 1 1-5.9-9.1" />
    <path d="M9 11l3 3L22 4" />
  </Svg>
);

/* The workspace / service-map glyph (S-250, FR-UI-29): three services wired
   across a boundary — the cross-service axis, distinct from the intra-repo
   IconGraph's free-form node cloud. */
export const IconWorkspace = (p: IconProps) => (
  <Svg {...p}>
    <rect x="3" y="3" width="7" height="6" rx="1.5" />
    <rect x="14" y="15" width="7" height="6" rx="1.5" />
    <rect x="3" y="15" width="7" height="6" rx="1.5" />
    <path d="M6.5 9v6M10 6h4.5a3 3 0 0 1 3 3v6" />
  </Svg>
);

export const IconConfig = (p: IconProps) => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="3" />
    <path d="M12 2v3M12 19v3M2 12h3M19 12h3M5 5l2 2M17 17l2 2M19 5l-2 2M7 17l-2 2" />
  </Svg>
);

export const IconStatistics = (p: IconProps) => (
  <Svg {...p}>
    <path d="M3 3v18h18" />
    <rect x="7" y="12" width="3" height="6" />
    <rect x="12" y="8" width="3" height="10" />
    <rect x="17" y="4" width="3" height="14" />
  </Svg>
);

export const IconSun = (p: IconProps) => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="4" />
    <path d="M12 2v2M12 20v2M2 12h2M20 12h2M5 5l1.5 1.5M17.5 17.5 19 19M19 5l-1.5 1.5M6.5 17.5 5 19" />
  </Svg>
);

export const IconMoon = (p: IconProps) => (
  <Svg {...p}>
    <path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" />
  </Svg>
);
