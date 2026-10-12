//! The binary's composition: the extensions it registers and the
//! feature gate it supplies ([FR-DS-04], [ADR-74], [BR-63]).
//!
//! It registers the `wiki` extension under the default-on `wiki` feature
//! ([S-644]) — with the in-app generator (`wiki-agent`) under `agents` — and the
//! `chat` extension under `agents` ([S-643]). It supplies no feature gate, and
//! neither extension names a feature, so both are active whenever registered.
//! Without either feature it registers nothing, and its surface is the core's
//! alone.
//!
//! Its own file so the surface snapshots (`cli/tests/surface_snapshots.rs`, which
//! compile it too) and the library's CLI↔MCP rosters read the very host `main`
//! composes rather than a copy of it: an extension registered here is read
//! through it by every snapshot.
//!
//! [FR-DS-04]: ../../docs/specs/requirements/FR-DS-04.md
//! [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md
//! [BR-63]: ../../docs/specs/software-spec.md#320-distribution
//! [S-643]: ../../docs/planning/journal.md#s-643-chat-is-a-registered-extension
//! [S-644]: ../../docs/planning/journal.md#s-644-the-wiki-is-its-own-crate-and-a-registered-extension

/// The host the public `logos` binary runs with.
///
/// Registration order is the order `GET /api/v1/extensions` lists the extensions in,
/// and so the order the SPA places two sidebar entries anchored to the same one:
/// chat first, then the wiki, both after Graph — where the built-in entries sat.
pub fn host() -> logos_ext::ExtensionHost {
    let host = logos_ext::ExtensionHost::new();
    #[cfg(feature = "agents")]
    let host = host.register(logos_chat::ChatExtension::new()).expect("registered once");
    // `agents` implies `wiki`: with it the wiki generates in-app, through the
    // `wiki-agent`'s configured generator; without it the wiki is listen-only.
    #[cfg(all(feature = "wiki", not(feature = "agents")))]
    let host = host.register(logos_wiki::WikiExtension::new()).expect("registered once");
    #[cfg(feature = "agents")]
    let host = host
        .register(logos_wiki::WikiExtension::with_generator(std::sync::Arc::new(wiki_agent::ConfiguredGenerator)))
        .expect("registered once");
    host
}
