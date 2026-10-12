//! `logos` — the binary's composition root ([FR-DS-04], [ADR-74]).
//!
//! The one place that names extensions: a binary composes its optional
//! capabilities ([`composition`]) and hands the host to the `logos_cli` library.
//! The binary registers the `wiki` extension under the default-on `wiki`
//! feature and the `chat` extension under `agents`, and supplies no feature gate,
//! so every extension it registers is active ([BR-63]).
//!
//! [FR-DS-04]: ../../docs/specs/requirements/FR-DS-04.md
//! [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md
//! [BR-63]: ../../docs/specs/software-spec.md#320-distribution

mod composition;

fn main() {
    logos_cli::run(composition::host())
}
