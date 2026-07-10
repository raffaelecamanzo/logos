//! The declarative plugin substrate ([plugin-registry], [ADR-09]).
//!
//! Logos binds compiled-in tree-sitter grammars to declarative
//! `plugin.toml` + `.scm` assets. This module is the *micro-kernel* that
//! realises that binding ([ADR-09]):
//!
//! - [`grammars`] — the cargo-feature-gated table of compiled-in grammars, each
//!   pairing embedded assets with a `tree_sitter_language::LanguageFn`
//!   ([FR-PL-01]). The `LanguageFn` indirection is what resolves the
//!   duplicate-symbol grammar-linking hazard ([NFR-PC-05], [AR-04],
//!   tree-sitter #4209) and decouples each grammar's ABI from the workspace
//!   ([NFR-PC-03]).
//! - [`manifest`] — the parsed `plugin.toml` descriptor ([FR-PL-02]).
//! - [`abi`] — the load-time ABI assertion; a mismatch is skipped-and-warned,
//!   never fatal ([FR-PL-03]).
//! - [`queries`] — query resolution (on-disk override shadows embedded,
//!   [FR-PL-04]/[FR-PL-05]) and fail-fast compilation ([FR-PL-02]).
//! - [`plugin`] — the [`LanguagePlugin`] trait the [extraction-engine] consumes.
//! - [`registry`] — the [`LanguageRegistry`] that loads, indexes by extension,
//!   and lists grammars ([FR-PL-06]).
//!
//! # Adding a language ([NFR-MA-01])
//!
//! For an already-compiled grammar crate: add a `plugins/<lang>/` asset dir, a
//! cargo feature line, and one `cfg`-gated row in [`grammars::compiled`] — no
//! other `logos-core` source changes. Tuning an existing grammar's *queries*
//! needs no rebuild at all: drop a `.scm` under `.logos/plugins/<lang>/queries/`
//! ([FR-PL-05]). Shadowing the descriptor's *semantics* on disk ([NFR-MA-05]) is
//! the next increment.
//!
//! [plugin-registry]: ../../../docs/specs/architecture/components/plugin-registry.md
//! [extraction-engine]: ../../../docs/specs/architecture/components/extraction-engine.md
//! [ADR-09]: ../../../docs/specs/architecture/decisions/ADR-09.md
//! [FR-PL-01]: ../../../docs/specs/requirements/FR-PL-01.md
//! [FR-PL-02]: ../../../docs/specs/requirements/FR-PL-02.md
//! [FR-PL-03]: ../../../docs/specs/requirements/FR-PL-03.md
//! [FR-PL-04]: ../../../docs/specs/requirements/FR-PL-04.md
//! [FR-PL-05]: ../../../docs/specs/requirements/FR-PL-05.md
//! [FR-PL-06]: ../../../docs/specs/requirements/FR-PL-06.md
//! [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
//! [NFR-MA-05]: ../../../docs/specs/requirements/NFR-MA-05.md
//! [NFR-PC-03]: ../../../docs/specs/requirements/NFR-PC-03.md
//! [NFR-PC-05]: ../../../docs/specs/requirements/NFR-PC-05.md
//! [AR-04]: ../../../docs/specs/architecture.md

pub mod abi;
pub mod error;
pub mod grammars;
pub mod manifest;
#[allow(clippy::module_inception)]
pub mod plugin;
pub mod queries;
pub mod registry;

pub use abi::AbiRange;
pub use error::{PluginError, SkipReason, SkippedGrammar};
pub use manifest::{
    AnchorDescriptor, ConfigDescriptor, ExportConvention, PluginManifest, TestConvention,
};
pub use plugin::{CompiledPlugin, LanguagePlugin, Semantics};
pub use registry::LanguageRegistry;
