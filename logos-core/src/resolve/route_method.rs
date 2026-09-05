//! The shared **wildcard-method matching rule** for HTTP endpoints (S-349,
//! [CR-109], [FR-CG-09]).
//!
//! [`route_template`](super::route_template) decides whether two endpoints have
//! the same *shape*; this module decides whether a provider registered under one
//! HTTP method **serves** a consumer that declares another, and which of the
//! providers sharing a template a consumer may bind.
//!
//! # Why the rule cannot live in the key
//!
//! Framework extraction renders an all-verbs registration — a Spring
//! `@RequestMapping` with no `method =`, an Express `app.all`/`app.use` mount —
//! with the wildcard method token [`WILDCARD_METHOD`], because that registration
//! genuinely serves every verb. A provider index keyed on the whole
//! `(method, template)` tuple can never match it: tuple equality has no notion of
//! a wildcard, so 92 of 105 normalizable providers in the measured workspace
//! matched nothing. The fix is therefore a change to the **index shape** — every
//! candidate-selection site buckets its providers on the normalized template
//! **alone** and resolves the method here, over that bucket's candidates
//! ([FR-WS-04], [ADR-52]).
//!
//! # The rule ([FR-CG-09])
//!
//! 1. **Compatibility** — a provider serves a consumer when their methods are
//!    equal, or when the provider's method is [`WILDCARD_METHOD`]. The wildcard is
//!    a *provider-side* concept: a route that answers every verb. A consumer's
//!    method is never widened, so a method mismatch still never binds.
//! 2. **Specificity** — ranking is total on exactly **one** axis, wildcard vs
//!    concrete: when a bucket holds both, only the concrete providers survive.
//!    This mirrors the framework's own dispatch precedence and is what makes the
//!    change safe for references binding today — a `GET` consumer that binds a
//!    `GET` provider cannot be pushed into ambiguity by a wildcard sibling.
//! 3. **Nothing else** — specificity is never a general tie-break. Two concrete
//!    providers, or two wildcard providers, of one template survive together and
//!    the caller's exactly-one gate then refuses them both ([NFR-RA-05]).
//!
//! The three candidate-selection sites — the intra-repo binder, the cross-member
//! bridge, and the federation coverage read-model — all reduce their bucket
//! through [`preferred_candidates`](self::preferred_candidates), so "why did
//! this bind" and "why didn't this bind" can never drift apart ([ADR-52]).
//!
//! # Namespaces without a method dimension
//!
//! The facet is an [`Option`] so the same reduction serves the gRPC and
//! broker-topic namespaces, whose keys carry no method at all: `None` is
//! compatible only with `None` and is maximally specific, so a bucket of them
//! survives whole — exactly the behaviour those namespaces had before this rule
//! existed.
//!
//! [CR-109]: ../../../docs/requests/CR-109-wildcard-method-route-matching.md
//! [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
//! [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md

/// The wildcard method token an all-verbs framework registration is rendered
/// with — the value each plugin descriptor's `[framework_methods]` table maps a
/// bare `@RequestMapping`, an Express `all`/`use` mount, a Django `path`, … to
/// ([FR-FW-05]). Extraction names the wildcard; this module interprets it.
///
/// [FR-FW-05]: ../../../docs/specs/requirements/FR-FW-05.md
pub(crate) const WILDCARD_METHOD: &str = "ANY";

/// Whether a provider registered under `provider` serves a request a consumer
/// makes with `consumer` — equal methods, or a [wildcard](WILDCARD_METHOD)
/// provider ([FR-CG-09]).
///
/// Deliberately **asymmetric**: the wildcard widens the provider side only. A
/// consumer never claims to speak every verb, so a `GET` consumer still never
/// reaches a `POST` provider — "a method mismatch never binds" is untouched.
///
/// [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
pub(crate) fn serves(provider: Option<&str>, consumer: Option<&str>) -> bool {
    provider == consumer || provider == Some(WILDCARD_METHOD)
}

/// A provider's rank on the single wildcard-vs-concrete specificity axis: a
/// concrete method outranks the wildcard, and nothing else is ever compared.
///
/// A facet-less namespace (`None`) ranks concrete, so a bucket of gRPC or
/// broker candidates is never thinned by this rule.
fn specificity(provider: Option<&str>) -> u8 {
    u8::from(provider != Some(WILDCARD_METHOD))
}

/// Reduce one template bucket to the candidates a consumer declaring `consumer`
/// may bind: the compatible providers of the **highest specificity present**
/// ([FR-CG-09]).
///
/// Returns them in input order, so a caller that files its bucket in a
/// deterministic order gets a deterministic candidate list ([NFR-RA-06]). An
/// empty result means the bucket holds no provider of this endpoint at all — the
/// same answer as an absent bucket, which is what keeps a method mismatch
/// reporting exactly as it did before wildcards existed.
///
/// This function never picks a winner: it narrows the bucket on the one
/// specificity axis and hands the survivors back for the caller's own
/// exactly-one gate to accept or refuse ([NFR-RA-05]).
///
/// [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub(crate) fn preferred_candidates<'a, T: 'a>(
    bucket: impl IntoIterator<Item = (Option<&'a str>, &'a T)>,
    consumer: Option<&str>,
) -> Vec<&'a T> {
    let mut best: Option<u8> = None;
    let mut winners: Vec<&'a T> = Vec::new();
    for (provider, value) in bucket {
        if !serves(provider, consumer) {
            continue;
        }
        let rank = specificity(provider);
        match best {
            // A strictly less specific provider than one already seen: the
            // exact-method rule drops it from the candidate set entirely.
            Some(seen) if rank < seen => continue,
            // Equally specific: both stand, and the caller's exactly-one gate
            // refuses them — specificity is never a general tie-break.
            Some(seen) if rank == seen => winners.push(value),
            // The first compatible provider, or a strictly more specific one that
            // supersedes everything collected so far.
            _ => {
                best = Some(rank);
                winners.clear();
                winners.push(value);
            }
        }
    }
    winners
}

#[cfg(test)]
pub(crate) mod matrix;

#[cfg(test)]
mod tests {
    use super::*;

    /// Compatibility is equality widened by a **provider-side** wildcard only
    /// ([FR-CG-09]): `ANY` serves every verb, but no verb reaches a differently
    /// registered concrete provider.
    #[test]
    fn a_wildcard_provider_serves_every_verb_and_a_mismatch_still_never_binds() {
        for verb in ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"] {
            assert!(
                serves(Some(WILDCARD_METHOD), Some(verb)),
                "an ANY provider serves {verb}"
            );
            assert!(serves(Some(verb), Some(verb)), "{verb} serves itself");
        }
        assert!(
            !serves(Some("POST"), Some("GET")),
            "a method mismatch never binds (FR-CG-09)"
        );
        // The wildcard is not widened on the consumer side: a consumer that
        // somehow declares ANY reaches only an ANY provider.
        assert!(!serves(Some("GET"), Some(WILDCARD_METHOD)));
        assert!(serves(Some(WILDCARD_METHOD), Some(WILDCARD_METHOD)));
    }

    /// A facet-less namespace (gRPC, broker topic) is unaffected: `None` matches
    /// `None` and ranks concrete, so its buckets survive whole.
    #[test]
    fn a_namespace_without_a_method_facet_is_untouched() {
        assert!(serves(None, None));
        assert!(!serves(None, Some("GET")));
        assert!(!serves(Some("GET"), None));
        let bucket = [(None, &1), (None, &2)];
        assert_eq!(
            preferred_candidates(bucket.iter().map(|(m, v)| (*m, *v)), None),
            [&1, &2],
            "a facet-less bucket is never thinned"
        );
    }

    /// The precedence rule: a concrete provider is the **sole** candidate when it
    /// shares a template with a wildcard one — the regression guard that makes
    /// wildcarding safe for references binding today.
    #[test]
    fn a_concrete_provider_is_the_sole_candidate_beside_a_wildcard() {
        let bucket = [(Some(WILDCARD_METHOD), &1), (Some("GET"), &2)];
        assert_eq!(
            preferred_candidates(bucket.iter().map(|(m, v)| (*m, *v)), Some("GET")),
            [&2],
            "the exact-method provider outranks the wildcard one"
        );
        // Order-independent: the wildcard arriving second must not win.
        let reversed = [(Some("GET"), &2), (Some(WILDCARD_METHOD), &1)];
        assert_eq!(
            preferred_candidates(reversed.iter().map(|(m, v)| (*m, *v)), Some("GET")),
            [&2]
        );
        // A *different* concrete method is not compatible, so it never displaces
        // the wildcard that genuinely serves this consumer.
        let other_verb = [(Some(WILDCARD_METHOD), &1), (Some("POST"), &2)];
        assert_eq!(
            preferred_candidates(other_verb.iter().map(|(m, v)| (*m, *v)), Some("GET")),
            [&1],
            "an incompatible concrete provider does not outrank a serving wildcard"
        );
    }

    /// Specificity is total on one axis and never a general tie-break: equally
    /// specific providers all survive, for the caller's exactly-one gate to refuse
    /// ([NFR-RA-05]).
    #[test]
    fn equally_specific_providers_all_survive() {
        let wildcards = [(Some(WILDCARD_METHOD), &1), (Some(WILDCARD_METHOD), &2)];
        assert_eq!(
            preferred_candidates(wildcards.iter().map(|(m, v)| (*m, *v)), Some("GET")),
            [&1, &2],
            "two wildcard providers stay ambiguous"
        );
        let concretes = [(Some("GET"), &1), (Some("GET"), &2)];
        assert_eq!(
            preferred_candidates(concretes.iter().map(|(m, v)| (*m, *v)), Some("GET")),
            [&1, &2],
            "two exact-method providers stay ambiguous, exactly as today"
        );
    }

    /// Every all-verbs registration a shipped descriptor declares, named
    /// positively — the closed list this guard must classify.
    ///
    /// A guard that only rejects *known-wrong* spellings cannot notice an entry
    /// that drifts to some third spelling, or one that is deleted outright. So
    /// the expectation is stated forwards: these tokens exist and map to
    /// [`WILDCARD_METHOD`]. Express `all`/`use` are in the list because they
    /// carry the rule beyond the JVM — an `app.all("/x", h)` mount is a wildcard
    /// provider under exactly the same rule a bare `@RequestMapping` is
    /// ([CR-109] §4.4).
    const WILDCARD_ENTRIES: &[(&str, &[&str])] = &[
        ("java/", &["RequestMapping"]),
        ("kotlin/", &["RequestMapping"]),
        ("typescript/", &["all", "use"]),
        ("tsx/", &["all", "use"]),
        ("python/", &["path", "re_path", "websocket"]),
        ("go/", &["Any", "Handle", "HandleFunc"]),
        ("c-sharp/", &["Route"]),
        ("php/", &["any", "match"]),
    ];

    /// The extraction side names the wildcard, this side interprets it
    /// ([FR-FW-05], [FR-CG-09]) — so every shipped descriptor's all-verbs entry
    /// must be this module's token verbatim. A descriptor that drifted to `*`,
    /// `any`, `ALL`, or that dropped the entry, would emit routes no consumer
    /// could ever reach — silently, because nothing else in the tree compares the
    /// two spellings.
    ///
    /// Two halves, because either alone is porous: [`WILDCARD_ENTRIES`] asserts
    /// **forwards** that each named token is present and spells `ANY`, and the
    /// sweep asserts **backwards** that no *other* entry means all-verbs while
    /// spelling it differently. A language whose feature is off is skipped, and
    /// the closing assertion proves at least one prefix was actually reached, so
    /// the whole test can never pass vacuously.
    #[test]
    fn every_shipped_descriptor_names_the_wildcard_with_the_shared_token() {
        use crate::plugin::{grammars, PluginManifest};

        let mut prefixes_seen = 0;
        for entry in grammars::compiled() {
            let manifest = PluginManifest::parse(entry.manifest_label, entry.manifest_toml)
                .expect("a shipped descriptor parses");

            // Forwards: every declared all-verbs token is present and spells `ANY`.
            for (prefix, tokens) in WILDCARD_ENTRIES {
                if !entry.manifest_label.starts_with(prefix) {
                    continue;
                }
                prefixes_seen += 1;
                for token in *tokens {
                    assert_eq!(
                        manifest.framework_methods.get(*token).map(String::as_str),
                        Some(WILDCARD_METHOD),
                        "{}: `{token}` must map to the wildcard `{WILDCARD_METHOD}` \
                         (drop it here only when the descriptor deliberately stops \
                         registering all verbs)",
                        entry.manifest_label
                    );
                }
            }

            // Backwards: no other entry may mean all-verbs under another spelling.
            for (token, method) in &manifest.framework_methods {
                let means_all_verbs = method == "*"
                    || method.eq_ignore_ascii_case("any")
                    || method.eq_ignore_ascii_case("all")
                    || method.eq_ignore_ascii_case("wildcard");
                assert!(
                    !means_all_verbs || method == WILDCARD_METHOD,
                    "{}: `{token} = \"{method}\"` must spell the wildcard `{WILDCARD_METHOD}`",
                    entry.manifest_label
                );
            }
        }
        assert!(
            prefixes_seen > 0,
            "no shipped descriptor matched a WILDCARD_ENTRIES prefix — the guard \
             checked nothing (a renamed manifest label, or every language feature off)"
        );
    }

    /// A bucket holding no compatible provider yields nothing — the same answer
    /// an absent bucket gives, so a method mismatch reports as it always has.
    #[test]
    fn a_bucket_with_no_compatible_provider_yields_nothing() {
        let bucket = [(Some("POST"), &1), (Some("DELETE"), &2)];
        assert!(
            preferred_candidates(bucket.iter().map(|(m, v)| (*m, *v)), Some("GET")).is_empty(),
            "no provider of this endpoint — not a fabricated one"
        );
    }
}
