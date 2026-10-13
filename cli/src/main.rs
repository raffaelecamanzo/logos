//! `logos` — the binary's composition root ([FR-DS-04], [ADR-74]).
//!
//! The one place that names extensions: a binary composes its optional
//! capabilities ([`composition`]) and hands the host to the `logos_cli` library.
//! This binary registers none ([FR-DS-05]); a binary that composes extensions
//! over these crates registers them here, and every extension it registers is
//! active unless a feature gate it supplies says otherwise ([BR-63]).
//!
//! [FR-DS-04]: ../../docs/specs/requirements/FR-DS-04.md
//! [FR-DS-05]: ../../docs/specs/requirements/FR-DS-05.md
//! [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md
//! [BR-63]: ../../docs/specs/software-spec.md#320-distribution

mod composition;

fn main() {
    logos_cli::run(composition::host())
}
