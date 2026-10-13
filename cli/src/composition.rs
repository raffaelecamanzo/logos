//! The binary's composition: the extensions it registers and the
//! feature gate it supplies ([FR-DS-04], [ADR-74], [BR-63]).
//!
//! This repository's binary registers no extension and supplies no feature gate,
//! so its surface is the core's alone: no chat or wiki route, tool, command, view
//! or config panel ([FR-DS-05]). The host stays the seam a binary composing
//! extensions over these crates registers them through.
//!
//! Its own file so the surface snapshots (`cli/tests/surface_snapshots.rs`, which
//! compile it too) and the library's CLI↔MCP rosters read the very host `main`
//! composes rather than a copy of it: an extension registered here is read
//! through it by every snapshot.
//!
//! [FR-DS-04]: ../../docs/specs/requirements/FR-DS-04.md
//! [FR-DS-05]: ../../docs/specs/requirements/FR-DS-05.md
//! [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md
//! [BR-63]: ../../docs/specs/software-spec.md#320-distribution

/// The host the public `logos` binary runs with: empty.
///
/// Registration order is the order `GET /api/v1/extensions` lists the extensions in,
/// and so the order the SPA places two sidebar entries anchored to the same one.
pub fn host() -> logos_ext::ExtensionHost {
    logos_ext::ExtensionHost::new()
}
