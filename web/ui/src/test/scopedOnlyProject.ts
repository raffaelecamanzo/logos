/*
 * The one fixture behind S-760's acceptance: a project whose only report is a
 * `--path` scan, so its scanner reads stale (`scoped_only`) and no report is
 * fresh — yet its two findings sit in a file nothing has touched, so each is
 * fresh and every security surface reads the same two. The Rust side holds the
 * same project (`security::surface_tests`); these are the figures the surfaces
 * are handed for it, so a surface that re-derived freshness from its report
 * would read something else.
 */

import type { SecurityExcluded, SecuritySeverityCounts } from "../api/types.ts";

/** Two medium findings, counted. */
export const SCOPED_ONLY_COUNTED: SecuritySeverityCounts = { critical: 0, high: 0, medium: 2, low: 0, info: 0, unrated: 0, total: 2 };

/** None left out. */
export const SCOPED_ONLY_EXCLUDED: SecurityExcluded = { not_fresh: 0, suppressed: 0 };
