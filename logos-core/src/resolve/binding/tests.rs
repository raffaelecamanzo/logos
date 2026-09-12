//! Unit tests for configuration-bound operand resolution (S-382).
//!
//! The refusal suite is the load-bearing half: [FR-WS-08]'s criterion names five
//! populations that must keep refusing, and each gets its own case here even
//! where two share a [`ValueRefusal`] variant — the population is what must not
//! regress, and a shared reason is not a shared mechanism.
//!
//! [FR-WS-08]: ../../../../docs/specs/requirements/FR-WS-08.md

use std::collections::BTreeMap;

use super::*;

/// A committed definition: file, profile, value.
fn def(path: &str, profile: Option<&str>, value: &str) -> ConfigDefinition {
    ConfigDefinition {
        path: path.to_string(),
        profile: profile.map(str::to_string),
        value: value.to_string(),
    }
}

/// A corpus standing in for the ingested `config_values` table: canonical key →
/// its committed definitions. Keys are canonicalised on insert, exactly as the
/// store holds them, so a test that spells a key in its relaxed source form
/// exercises the same canonicalisation production does.
#[derive(Default)]
struct Corpus(BTreeMap<String, Vec<ConfigDefinition>>);

impl Corpus {
    fn with(mut self, key: &str, defs: &[ConfigDefinition]) -> Self {
        self.0.insert(canonical_key(key), defs.to_vec());
        self
    }
}

impl ConfigLookup for Corpus {
    fn definitions(&self, key: &str, _module: &str) -> Vec<ConfigDefinition> {
        self.0.get(key).cloned().unwrap_or_default()
    }
}

/// A resolver over `corpus` with no configuration-bound classes — the accessor
/// half is S-381's and is exercised in its own module; every case here starts
/// from a key that is already named.
fn resolver<'a>(corpus: &'a Corpus, props: &'a PropertiesIndex) -> Resolver<'a> {
    Resolver { corpus, props, module: "" }
}

// ── AC1: an admitted value carries its key, sources and profile set ─────────

/// AC1. A key one committed source defines resolves to its value, and the
/// provenance names the key, the defining file and the profile — not just the
/// value it produced.
#[test]
fn a_committed_key_resolves_and_carries_its_key_sources_and_profiles() {
    let corpus = Corpus::default().with(
        "orders.api.base-path",
        &[def("svc/src/main/resources/application-docker.yml", Some("docker"), "/orders/v1")],
    );
    let props = PropertiesIndex::default();
    let bound = resolver(&corpus, &props)
        .resolve("orders.api.base-path", KeySource::Properties)
        .expect("a committed key admits");

    // Canonical, so the relaxed source spelling and the stored key are one key.
    assert_eq!(bound.key, "orders.api.basepath");
    assert_eq!(bound.source, KeySource::Properties);
    assert_eq!(bound.values.len(), 1);
    assert_eq!(bound.values[0].value, "/orders/v1");
    assert_eq!(bound.values[0].profiles, ["docker"]);
    assert!(!bound.values[0].unprofiled);
    assert_eq!(bound.values[0].sources, ["svc/src/main/resources/application-docker.yml"]);
    assert_eq!(bound.profiles(), ["docker"]);
    assert!(!bound.is_divergent());
}

/// AC1. The relaxed source spelling of a key reaches the same committed value as
/// its canonical spelling — the near miss being that `uriGetArchive` and
/// `uri-get-archive` would otherwise be two keys and the second would refuse as
/// missing.
#[test]
fn a_relaxed_spelling_reaches_the_same_committed_value() {
    let corpus = Corpus::default()
        .with("mailserver.api.uri-get-archive", &[def("application.yml", None, "/archive")]);
    let props = PropertiesIndex::default();
    let r = resolver(&corpus, &props);
    for spelling in ["mailserver.api.uri-get-archive", "mailserver.api.uriGetArchive"] {
        let bound = r.resolve(spelling, KeySource::Properties).expect("both spellings admit");
        assert_eq!(bound.values[0].value, "/archive", "{spelling} must reach the committed value");
    }
}

/// AC1/AC4. An admitted value and a directly-observed literal are the same type
/// and differ by one wire word, so no surface can carry one without being able
/// to carry the other.
#[test]
fn an_admitted_value_is_tagged_apart_from_an_observed_literal() {
    let corpus = Corpus::default().with("a.b", &[def("application.yml", None, "/x")]);
    let props = PropertiesIndex::default();
    let bound = resolver(&corpus, &props).resolve("a.b", KeySource::Placeholder).expect("admits");

    let observed = Provenance::Literal;
    let admitted = Provenance::ConfigBound(bound);
    assert_eq!(observed.label(), "literal");
    assert_eq!(admitted.label(), "config-bound");
    assert!(observed.config_bound().is_none());
    assert!(admitted.config_bound().is_some());

    let observed_json = serde_json::to_value(&observed).expect("literal serializes");
    let admitted_json = serde_json::to_value(&admitted).expect("admitted serializes");
    assert_eq!(observed_json["provenance"], "literal");
    assert_eq!(admitted_json["provenance"], "config-bound");
    // The evidence rides on the same object, not on a sibling a consumer must
    // know to fetch ([NFR-CC-04]).
    assert_eq!(admitted_json["key"], "a.b");
    assert_eq!(admitted_json["values"][0]["value"], "/x");
    assert_eq!(admitted_json["values"][0]["unprofiled"], true);
    // The literal carries no evidence keys at all, so a consumer switching on
    // `provenance` can never read a stale one.
    assert!(observed_json.get("key").is_none());
}

// ── AC2: overlay divergence is retained, not averaged and not refused ───────

/// AC2. A key two overlays define differently admits **both** values, each
/// tagged with the profile that proves it — the rule that differs from the
/// superseded S-366 reading, which refused here.
#[test]
fn overlay_disagreement_retains_every_value_with_its_profile() {
    let corpus = Corpus::default().with(
        "orders.base",
        &[
            def("application.yml", None, "/orders"),
            def("application-docker.yml", Some("docker"), "/orders-docker"),
            def("application-it.yml", Some("it"), "/orders-it"),
        ],
    );
    let props = PropertiesIndex::default();
    let bound =
        resolver(&corpus, &props).resolve("orders.base", KeySource::Properties).expect("admits");

    assert!(bound.is_divergent());
    assert_eq!(bound.values.len(), 3, "every overlay's value reaches the consumer");
    let seen: Vec<(&str, Vec<&str>, bool)> = bound
        .values
        .iter()
        .map(|v| (v.value.as_str(), v.profiles.iter().map(String::as_str).collect(), v.unprofiled))
        .collect();
    assert_eq!(
        seen,
        [
            ("/orders", vec![], true),
            ("/orders-docker", vec!["docker"], false),
            ("/orders-it", vec!["it"], false),
        ]
    );
    assert_eq!(bound.profiles(), ["docker", "it"]);
}

/// AC2. Two profiles committing the *same* value are one value proving two
/// profiles, not two values — the agreement rule counts distinct values, and an
/// estate that repeats one host across overlays must not read as divergent.
#[test]
fn profiles_that_agree_collapse_to_one_value_naming_both() {
    let corpus = Corpus::default().with(
        "a.b",
        &[
            def("application-docker.yml", Some("docker"), "/same"),
            def("application-it.yml", Some("it"), "/same"),
        ],
    );
    let props = PropertiesIndex::default();
    let bound = resolver(&corpus, &props).resolve("a.b", KeySource::Properties).expect("admits");
    assert!(!bound.is_divergent());
    assert_eq!(bound.values[0].profiles, ["docker", "it"]);
    assert_eq!(bound.values[0].sources.len(), 2);
    assert!(matches!(Agreement::of(&corpus.definitions("a.b", "")), Agreement::Agreed(_)));
}

/// AC2. A divergent key composes **one template per overlay**, and every one is
/// reachable from the consumer of the resolution — the composition half of "every
/// one reaches the consumer".
#[test]
fn a_divergent_key_composes_one_template_per_overlay() {
    let corpus = Corpus::default().with(
        "orders.base",
        &[
            def("application.yml", None, "/orders"),
            def("application-docker.yml", Some("docker"), "/orders-docker"),
        ],
    );
    let props = PropertiesIndex::default();
    let resolved = resolver(&corpus, &props)
        .resolve_template("${orders.base}/{id}")
        .expect("the template carries a placeholder")
        .expect("it admits");

    let rendered: Vec<(&str, Vec<&str>, bool)> = resolved
        .candidates
        .iter()
        .map(|c| {
            (c.template.as_str(), c.profiles.iter().map(String::as_str).collect(), c.unprofiled)
        })
        .collect();
    assert_eq!(
        rendered,
        [("/orders-docker/{id}", vec!["docker"], false), ("/orders/{id}", vec![], true)]
    );
    // The provenance travels with the composition, so a consumer holding a
    // candidate can always name the key behind it.
    assert_eq!(resolved.bound.len(), 1);
    assert_eq!(resolved.bound[0].key, "orders.base");
    assert!(resolved.bound[0].is_divergent());
}

/// AC2. A profile silent about a key inherits the unprofiled base — that is what
/// an overlay means — so a second placeholder does not multiply the candidate
/// set per profile.
#[test]
fn a_profile_silent_about_a_key_inherits_the_unprofiled_base() {
    let corpus = Corpus::default()
        .with(
            "svc.host",
            &[
                def("application.yml", None, "http://base"),
                def("application-docker.yml", Some("docker"), "http://docker"),
            ],
        )
        .with("svc.path", &[def("application.yml", None, "/v1")]);
    let props = PropertiesIndex::default();
    let resolved = resolver(&corpus, &props)
        .resolve_template("${svc.host}${svc.path}/orders")
        .expect("placeholders")
        .expect("admits");

    let rendered: Vec<&str> = resolved.candidates.iter().map(|c| c.template.as_str()).collect();
    assert_eq!(
        rendered,
        ["http://base/v1/orders", "http://docker/v1/orders"],
        "the docker overlay keeps the base path it is silent about"
    );
    assert_eq!(resolved.candidates.len(), 2, "one per profile, never a cross-product");
}

/// AC2. Two profiles that produce the **same** composition are one candidate
/// naming both, never two identical rows a consumer would render twice.
#[test]
fn profiles_composing_the_same_template_are_one_candidate() {
    let corpus = Corpus::default().with(
        "a.b",
        &[
            def("application-it.yml", Some("it"), "/x"),
            def("application-local.yml", Some("local"), "/x"),
        ],
    );
    let props = PropertiesIndex::default();
    let resolved =
        resolver(&corpus, &props).resolve_template("${a.b}/y").expect("ph").expect("admits");
    assert_eq!(resolved.candidates.len(), 1);
    assert_eq!(resolved.candidates[0].template, "/x/y");
    assert_eq!(resolved.candidates[0].profiles, ["it", "local"]);
    assert!(
        !resolved.candidates[0].unprofiled,
        "no unprofiled source proves it, and that is stated rather than inferred"
    );
}

// ── AC3: the refusal suite — one case per population ────────────────────────

/// AC3 (1/5). An environment variable with no committed default is refused
/// **even when a key of the same canonical spelling agrees**: `canonical_key`
/// lower-cases and strips separators, so `BASE_URL` and a yaml `base-url` are one
/// key, and admitting the read would report the repository as proving a value it
/// does not.
#[test]
fn an_uncommitted_environment_variable_is_refused_even_when_a_key_agrees() {
    let corpus = Corpus::default().with("BASE_URL", &[def("application.yml", None, "/proven")]);
    let props = PropertiesIndex::default();
    let r = resolver(&corpus, &props);
    assert_eq!(r.resolve("BASE_URL", KeySource::Environment), Err(ValueRefusal::Uncommitted));
    // The near miss that proves the guard is the SOURCE and not the spelling:
    // the very same key, reached from a committed source, admits.
    assert!(r.resolve("BASE_URL", KeySource::Properties).is_ok());
}

/// AC3 (2/5). A `getenv` read reaches resolution as an environment key source
/// and is refused, with nothing in the corpus at all — the plain case, kept apart
/// from the colliding-key one above because the two fail for different reasons
/// and only one of them is a near miss.
#[test]
fn a_getenv_read_is_refused_as_uncommitted() {
    let corpus = Corpus::default();
    let props = PropertiesIndex::default();
    assert_eq!(
        resolver(&corpus, &props).resolve("ORDERS_HOST", KeySource::Environment),
        Err(ValueRefusal::Uncommitted),
        "the repository commits no value, so nothing in the tree proves what it holds"
    );
}

/// AC3 (3/5). A config server, Consul/etcd, a secret store or a Kubernetes
/// ConfigMap supplies its value outside the repository, and is refused
/// **structurally**: no such file is a configuration source under the discovery
/// gate, so nothing it holds reaches the corpus and the key is missing.
///
/// The gate is asserted here rather than assumed, because that — and not a
/// dedicated refusal variant — is the whole of the mechanism.
#[test]
fn a_config_server_or_secret_store_value_never_reaches_the_corpus() {
    use crate::extract::config::corpus::config_profile;
    for name in [
        "bootstrap.yml",             // Spring Cloud Config's own bootstrap
        "configmap.yaml",            // a Kubernetes ConfigMap manifest
        "vault-agent.yml",           // a secret-store template
        "consul-config.properties",  // a Consul-sourced dump
    ] {
        assert!(
            config_profile(name).is_none(),
            "{name} must not be admitted as a configuration source"
        );
    }
    // So the key it would have supplied is committed nowhere the corpus admits.
    let corpus = Corpus::default();
    let props = PropertiesIndex::default();
    assert_eq!(
        resolver(&corpus, &props).resolve("orders.api.url", KeySource::Properties),
        Err(ValueRefusal::MissingKey),
    );
}

/// AC3 (4/5). A committed value that is itself an unresolved `${…}` indirection
/// proves the indirection, not the value — refused under its own reason, never
/// folded into "missing".
#[test]
fn a_value_that_is_itself_a_placeholder_proves_nothing() {
    let corpus = Corpus::default()
        .with("orders.url", &[def("application.yml", None, "${ORDERS_SERVICE_URL}")]);
    let props = PropertiesIndex::default();
    assert_eq!(
        resolver(&corpus, &props).resolve("orders.url", KeySource::Properties),
        Err(ValueRefusal::PlaceholderValue),
    );
    assert_eq!(Agreement::of(&corpus.definitions("orders.url", "")).label(), "placeholder value");
}

/// AC3 (4/5, the near miss). One source's value being a placeholder refuses the
/// key **even when another source proves a literal** — the sources do not agree
/// on a value at all, and preferring the literal would be the default-profile
/// guess in a new hat.
#[test]
fn one_placeholder_source_refuses_the_key_even_beside_a_literal_one() {
    let corpus = Corpus::default().with(
        "orders.url",
        &[
            def("application.yml", None, "/orders"),
            def("application-docker.yml", Some("docker"), "${ORDERS_URL}"),
        ],
    );
    let props = PropertiesIndex::default();
    assert_eq!(
        resolver(&corpus, &props).resolve("orders.url", KeySource::Properties),
        Err(ValueRefusal::PlaceholderValue),
    );
}

/// AC3 (5/5). A key no committed source defines is refused under a reason
/// **distinct from disagreement**, so the two are never conflated in a count —
/// and disagreement is no longer a refusal at all, which is what makes the
/// distinction cheap to keep.
#[test]
fn a_key_no_committed_source_defines_is_missing_not_a_disagreement() {
    let corpus = Corpus::default().with("other.key", &[def("application.yml", None, "/x")]);
    let props = PropertiesIndex::default();
    assert_eq!(
        resolver(&corpus, &props).resolve("orders.base", KeySource::Properties),
        Err(ValueRefusal::MissingKey),
    );
    assert_eq!(Agreement::of(&[]).label(), "missing key");
    // Disagreement is an admission, not a refusal — the S-382 rule change.
    let divergent = Agreement::of(&[
        def("application.yml", None, "/a"),
        def("application-it.yml", Some("it"), "/b"),
    ]);
    assert_eq!(divergent.label(), "profile-divergent");
    assert_eq!(divergent.values().len(), 2);
}

/// AC3. A template refuses as a whole when **any** of its placeholders refuses:
/// a composition whose parts are not all proven is not proven.
#[test]
fn a_template_refuses_when_any_placeholder_refuses() {
    let corpus = Corpus::default().with("a.b", &[def("application.yml", None, "/x")]);
    let props = PropertiesIndex::default();
    assert_eq!(
        resolver(&corpus, &props).resolve_template("${a.b}/${missing.key}"),
        Some(Err(ValueRefusal::MissingKey)),
    );
}

/// Every refusal variant carries a distinct census word, so a report can never
/// file two populations under one label.
#[test]
fn every_refusal_variant_has_a_distinct_label() {
    let value_labels: BTreeMap<&str, ValueRefusal> =
        ValueRefusal::ALL.iter().map(|r| (r.label(), *r)).collect();
    assert_eq!(value_labels.len(), ValueRefusal::ALL.len());
    let accessor_labels: BTreeMap<&str, Refusal> =
        Refusal::ALL.iter().map(|r| (r.label(), *r)).collect();
    assert_eq!(accessor_labels.len(), Refusal::ALL.len());
}

// ── The placeholder scanner, probed with its near misses ────────────────────

/// A route parameter is not a configuration placeholder. This is the near miss
/// that matters most: reading `{id}` as a key would turn every route template in
/// the estate into a configuration lookup.
#[test]
fn a_route_parameter_is_not_a_placeholder() {
    assert_eq!(placeholder_keys("/users/{id}/orders/{orderId}"), None);
    assert_eq!(placeholder_keys("/users"), None);
    // `$(…)` is shell substitution, not Spring's; `$` alone is a literal dollar.
    assert_eq!(placeholder_keys("/pay/$(amount)"), None);
    assert_eq!(placeholder_keys("/cost/$100"), None);
}

/// An unterminated `${` yields no key and is left verbatim — reading past it
/// would fabricate a key out of the rest of the path.
#[test]
fn an_unterminated_placeholder_yields_no_key() {
    assert_eq!(placeholder_keys("/a/${orders.base/b"), None);
    // A complete placeholder BEFORE an unterminated one still counts; the scan
    // stops at the broken one rather than discarding what it already proved.
    assert_eq!(
        placeholder_keys("${a.b}/x/${c.d"),
        Some(vec!["a.b".to_string()]),
        "the complete placeholder is read and the truncated one contributes nothing"
    );
}

/// A placeholder's inline default names the key, and the default itself is never
/// the value: it proves what the source falls back to, not what is deployed.
#[test]
fn an_inline_default_names_the_key_but_never_supplies_the_value() {
    assert_eq!(
        placeholder_keys("${orders.base:/fallback}/x"),
        Some(vec!["orders.base".to_string()])
    );
    let corpus = Corpus::default();
    let props = PropertiesIndex::default();
    assert_eq!(
        resolver(&corpus, &props).resolve_template("${orders.base:/fallback}/x"),
        Some(Err(ValueRefusal::MissingKey)),
        "an undefined key with a committed default is still undefined",
    );
}

/// Keys are collected in source order and deduplicated, and a template naming
/// one key twice substitutes both occurrences.
#[test]
fn a_repeated_key_is_read_once_and_substituted_everywhere() {
    assert_eq!(
        placeholder_keys("${a.b}/x/${c.d}/y/${a.b}"),
        Some(vec!["a.b".to_string(), "c.d".to_string()])
    );
    let corpus = Corpus::default()
        .with("a.b", &[def("application.yml", None, "P")])
        .with("c.d", &[def("application.yml", None, "Q")]);
    let props = PropertiesIndex::default();
    let resolved = resolver(&corpus, &props)
        .resolve_template("${a.b}/x/${c.d}/y/${a.b}")
        .expect("ph")
        .expect("admits");
    assert_eq!(resolved.candidates[0].template, "P/x/Q/y/P");
}

/// A relaxed placeholder spelling substitutes against the canonical key — the
/// same equivalence `resolve` applies, asserted through the substitution path
/// because that is a second place the two spellings must meet.
#[test]
fn a_relaxed_placeholder_spelling_substitutes_against_the_canonical_key() {
    let corpus =
        Corpus::default().with("orders.base-path", &[def("application.yml", None, "/orders")]);
    let props = PropertiesIndex::default();
    let resolved = resolver(&corpus, &props)
        .resolve_template("${orders.basePath}/{id}")
        .expect("ph")
        .expect("admits");
    assert_eq!(resolved.candidates[0].template, "/orders/{id}");
}

/// A template with no placeholder is not this module's business: `None`, so the
/// caller keeps its literal rather than receiving an empty admission it might
/// read as a refusal.
#[test]
fn a_literal_template_is_not_a_resolution_at_all() {
    let corpus = Corpus::default();
    let props = PropertiesIndex::default();
    assert!(resolver(&corpus, &props).resolve_template("/users/{id}").is_none());
}

/// An empty corpus admits nothing rather than admitting a guess — the honest
/// answer when a member's configuration could not be read at all.
#[test]
fn an_unreadable_corpus_admits_nothing() {
    let corpus = Corpus::default();
    let props = PropertiesIndex::default();
    assert_eq!(
        resolver(&corpus, &props).resolve("anything.at.all", KeySource::Properties),
        Err(ValueRefusal::MissingKey),
    );
}
