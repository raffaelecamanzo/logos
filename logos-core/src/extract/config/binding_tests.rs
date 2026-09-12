//! Unit tests for the generic configuration-binding interpreter (S-381).
//!
//! In their own file, like `corpus_tests.rs`/`tests.rs`/`proto_tests.rs` beside
//! them.
//!
//! Two of these are **falsifications rather than illustrations**, and they are
//! the ones the story turns on:
//! [`kotlin_binds_through_the_same_interpreter_with_no_core_edit`] adds a second
//! language's binding vocabulary as descriptor data and drives it, and
//! [`the_interpreter_names_no_jvm_grammar_node_kind`] proves structurally that
//! there is nowhere for a language-specific reading to hide.

use super::*;

use crate::plugin::LanguageRegistry;

/// The loaded registry, built once per test binary.
fn registry() -> &'static LanguageRegistry {
    static ONCE: std::sync::OnceLock<LanguageRegistry> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| LanguageRegistry::load(std::env::temp_dir()).expect("registry loads"))
}

fn plugin(ext: &str) -> &'static dyn LanguagePlugin {
    registry()
        .for_extension(ext)
        .unwrap_or_else(|| panic!("{ext} plugin"))
}

/// Index one source through one plugin and seal — the shape every case below
/// uses, so a case reads as its fixture plus its assertion.
fn index_one(ext: &str, rel: &str, module: &str, source: &str) -> PropertiesIndex {
    let mut index = PropertiesIndex::for_plugins(&[plugin(ext)]);
    index.absorb_source(plugin(ext), rel, module, source);
    index.seal();
    index
}

// ── AC1: the capability, the query and the descriptor section ───────────────

#[test]
fn the_java_descriptor_names_its_vocabulary_and_its_accessor_convention() {
    let java = plugin("java");
    assert!(
        java.capabilities().iter().any(|c| c == PROPERTIES_CAPABILITY),
        "the descriptor must declare the capability: {:?}",
        java.capabilities(),
    );
    assert!(
        java.query(PROPERTIES_CAPABILITY).is_some(),
        "…and ship a query the registry compiled for it",
    );
    let descriptor = java
        .semantics()
        .properties
        .as_ref()
        .expect("the [properties] descriptor section");
    assert_eq!(
        descriptor.annotations,
        ["ConfigurationProperties"],
        "the vocabulary is named in the descriptor, not in the query's predicates",
    );
    assert_eq!(
        descriptor.accessor_prefixes,
        ["get", "is"],
        "…and so is the accessor convention",
    );
}

#[test]
fn an_annotated_class_yields_its_prefix_its_name_and_its_declared_fields() {
    let index = index_one(
        "java",
        "src/MailServerConfigurationApi.java",
        "mailbox",
        r#"
        @ConfigurationProperties(prefix = "mailserver.api")
        public class MailServerConfigurationApi {
            private String uriGetArchive;
            private int timeout, retries;
            public String getUriGetArchive() { return uriGetArchive; }
        }
        "#,
    );
    let class = index
        .get("MailServerConfigurationApi", "mailbox")
        .expect("indexed");
    assert_eq!(class.name, "MailServerConfigurationApi");
    assert_eq!(class.prefix, "mailserver.api");
    assert_eq!(class.file, "src/MailServerConfigurationApi.java");
    assert_eq!(class.module, "mailbox");
    assert_eq!(
        class.properties,
        ["urigetarchive", "timeout", "retries"]
            .into_iter()
            .map(canonical_key)
            .collect::<BTreeSet<_>>(),
        "every declarator of a multi-declarator field is a property, and a \
         getter is not one",
    );
}

#[test]
fn the_annotation_value_form_carries_a_prefix_too() {
    let index = index_one(
        "java",
        "Ds.java",
        "",
        r#"@ConfigurationProperties("spring.datasource.batch")
           public class Ds { private String url; }"#,
    );
    assert_eq!(index.get("Ds", "").expect("indexed").prefix, "spring.datasource.batch");
}

#[test]
fn a_record_declares_its_components_and_not_a_helpers_parameter() {
    let index = index_one(
        "java",
        "ApiProps.java",
        "",
        r#"
        @ConfigurationProperties(prefix = "api")
        public record ApiProps(String baseUrl, String uriGet) {
            static String helper(String uriGetArchive) { return uriGetArchive; }
        }
        "#,
    );
    let class = index.get("ApiProps", "").expect("indexed");
    assert!(class.properties.contains(&canonical_key("baseUrl")));
    assert!(class.properties.contains(&canonical_key("uriGet")));
    assert!(
        !class.properties.contains(&canonical_key("uriGetArchive")),
        "a method parameter is not a bound property",
    );
}

#[test]
fn a_nested_types_fields_belong_to_the_nested_type() {
    let index = index_one(
        "java",
        "P.java",
        "",
        r#"
        @ConfigurationProperties(prefix = "api")
        public class P { private String a; static class Inner { private String b; } }
        "#,
    );
    let class = index.get("P", "").expect("indexed");
    assert!(class.properties.contains(&canonical_key("a")));
    assert!(
        !class.properties.contains(&canonical_key("b")),
        "a nested type's field is that type's property, reached through a \
         nested accessor this substrate refuses rather than guesses at",
    );
    assert!(index.get("Inner", "").is_none(), "the nested type binds nothing of its own");
}

// ── The near misses, each one character from matching ───────────────────────

#[test]
fn a_sibling_annotations_argument_is_not_the_classs_prefix() {
    // THE bug this rule exists for, and it is not hypothetical: a prefix read
    // per CLASS rather than per ANNOTATION took `@RequestMapping`'s path on the
    // reference estate, which dropped six classes to `prefixless` and lost one
    // simple-name collision with them.
    let index = index_one(
        "java",
        "P.java",
        "",
        r#"
        @Validated
        @RequestMapping(value = "/not-a-prefix")
        @ConfigurationProperties(prefix = "mailserver.api")
        @Component("mailServerBean")
        public class P { private String host; }
        "#,
    );
    let class = index.get("P", "").expect("indexed");
    assert_eq!(class.prefix, "mailserver.api");
    assert_eq!(index.prefixless, 0, "the class is keyed, not counted as prefixless");
}

#[test]
fn an_annotation_outside_the_vocabulary_binds_nothing() {
    let index = index_one(
        "java",
        "P.java",
        "",
        r#"@ConfigurationPropertiesScan(prefix = "mailserver.api")
           public class P { private String host; }"#,
    );
    // One character from the vocabulary row is still outside it: the descriptor
    // is matched EXACTLY, never by prefix or substring.
    assert!(index.is_empty(), "an unlisted annotation indexes no class");
    assert_eq!(index.prefixless, 0, "…and is not counted as a prefixless one either");
}

#[test]
fn a_non_literal_prefix_leaves_the_class_prefixless_rather_than_keyed() {
    let index = index_one(
        "java",
        "P.java",
        "",
        r#"@ConfigurationProperties(prefix = PREFIX)
           public class P { private String host; }"#,
    );
    assert!(index.is_empty(), "a prefix that is not a written literal keys nothing");
    assert_eq!(index.prefixless, 1, "…and the class is COUNTED, not silently dropped");
}

#[test]
fn the_named_value_alias_is_a_prefix_and_a_sibling_named_argument_is_not() {
    // `value` is Spring's alias for `prefix`, and pattern 2's `#any-of?` reads
    // both. Mutation-proved uncovered at review in BOTH directions: narrowing
    // the predicate to `"prefix"` alone and widening it to admit
    // `ignoreUnknownFields` each left the suite green.
    let index = index_one(
        "java",
        "P.java",
        "",
        r#"@ConfigurationProperties(value = "a.b", ignoreUnknownFields = false)
           public class P { private String host; }"#,
    );
    let class = index.get("P", "").expect("indexed");
    assert_eq!(class.prefix, "a.b", "the `value` key is the prefix");
    assert_eq!(index.prefixless, 0, "…and no other named argument competed with it");
}

#[test]
fn two_readable_prefixes_on_one_declaration_prove_neither() {
    // `@ConfigurationProperties(prefix = "a", value = "b")` is legal Java —
    // Spring rejects it only at runtime, through `@AliasFor` — so this refusal
    // is reachable, not defensive. It is counted with the prefixless, which the
    // `prefixless` field doc names as the third of its three causes.
    let index = index_one(
        "java",
        "P.java",
        "",
        r#"@ConfigurationProperties(prefix = "a", value = "b")
           public class P { private String host; }"#,
    );
    assert!(index.is_empty(), "neither prefix keys the class");
    assert_eq!(index.prefixless, 1, "…and the class is COUNTED, not dropped");

    // The same two keys agreeing is not a refusal: one distinct prefix survives.
    let agreed = index_one(
        "java",
        "P.java",
        "",
        r#"@ConfigurationProperties(prefix = "a", value = "a")
           public class P { private String host; }"#,
    );
    assert_eq!(agreed.get("P", "").expect("indexed").prefix, "a");
    assert_eq!(agreed.prefixless, 0);
}

#[test]
fn an_annotated_record_with_a_marker_annotation_is_prefixless_too() {
    let index = index_one(
        "java",
        "R.java",
        "",
        "@ConfigurationProperties
public record R(String host) {}",
    );
    assert!(index.is_empty());
    assert_eq!(index.prefixless, 1, "the record arm of pattern 1 reaches the marker form");
}

#[test]
fn a_marker_annotation_is_prefixless_rather_than_keyed_to_the_empty_prefix() {
    let index = index_one(
        "java",
        "P.java",
        "",
        "@ConfigurationProperties\npublic class P { private String host; }",
    );
    assert!(index.is_empty());
    assert_eq!(index.prefixless, 1);
}

// ── AC3: accessor → field → owning class → prefix → canonical key ────────────

#[test]
fn an_accessor_resolves_to_a_key_recording_its_owning_class_and_field() {
    let index = index_one(
        "java",
        "src/MailServerConfigurationApi.java",
        "mailbox",
        r#"
        @ConfigurationProperties(prefix = "mailserver.api")
        public class MailServerConfigurationApi { private String uriGetArchive; }
        "#,
    );
    let class = index.get("MailServerConfigurationApi", "mailbox").expect("indexed");
    let binding = index.bind(class, "getUriGetArchive").expect("binds");
    assert_eq!(binding.key, "mailserver.api.uriGetArchive");
    assert_eq!(
        canonical_key(&binding.key),
        "mailserver.api.urigetarchive",
        "the key the corpus is matched on is the relaxed-binding one",
    );
    assert_eq!(binding.property, canonical_key("uriGetArchive"), "the FIELD is recorded");
    assert_eq!(binding.class, "MailServerConfigurationApi", "…and the OWNING CLASS");
    assert_eq!(binding.file, "src/MailServerConfigurationApi.java");
}

#[test]
fn the_is_convention_binds_a_boolean_property() {
    let index = index_one(
        "java",
        "P.java",
        "",
        r#"@ConfigurationProperties(prefix = "api")
           public class P { private boolean enabled; }"#,
    );
    let class = index.get("P", "").expect("indexed");
    assert_eq!(index.bind(class, "isEnabled").expect("binds").key, "api.enabled");
}

#[test]
fn an_index_over_zero_classes_still_knows_what_an_accessor_looks_like() {
    // The conventions come from the plugins the index is DECLARED over, not
    // from the plugins that happened to contribute a class. When they came from
    // the latter, a corpus with no bound class refused every use site as "not an
    // accessor" — collapsing two different faults into one and moving the count
    // from the refusal a reader can act on to the one they cannot.
    let index = PropertiesIndex::for_plugins(&[plugin("java")]);
    assert!(index.is_empty(), "nothing absorbed");
    assert!(index.names_an_accessor("java", "getHost"));
    assert!(!index.names_an_accessor("java", "compute"));
}

#[test]
fn absorbing_a_source_adopts_its_plugins_convention_without_a_prior_declaration() {
    // `absorb_source` is the ingestion-shaped entry point S-382 will wire up, so
    // it has to be total: feeding it a source without `for_plugins` first must
    // still judge that source's accessors by its own language's convention.
    // Review mutation-proved this branch uncovered — deleting the `declare` call
    // inside `absorb_source` left the whole suite green.
    let mut index = PropertiesIndex::default();
    index.absorb_source(
        plugin("java"),
        "P.java",
        "",
        r#"@ConfigurationProperties(prefix = "api")
           public class P { private String host; }"#,
    );
    index.seal();
    let class = index.get("P", "").expect("indexed");
    assert_eq!(
        index.bind(class, "getHost").expect("binds").key,
        "api.host",
        "the convention arrived with the source, not from a prior declaration",
    );
}

#[test]
fn the_three_failures_of_an_accessor_are_counted_apart() {
    let index = index_one(
        "java",
        "P.java",
        "",
        r#"@ConfigurationProperties(prefix = "api")
           public class P { private String host; }"#,
    );
    let class = index.get("P", "").expect("indexed");
    assert_eq!(
        index.bind(class, "compute"),
        Err(BindingRefusal::NotAnAccessor),
        "a name no convention strips names no property",
    );
    assert_eq!(
        index.bind(class, "get"),
        Err(BindingRefusal::NotAnAccessor),
        "a bare `get` strips to the empty property, which is not a name",
    );
    assert_eq!(
        index.bind(class, "getPort"),
        Err(BindingRefusal::PropertyNotDeclared),
        "the convention applies, the class simply does not declare it",
    );
    assert!(index.names_an_accessor("java", "getPort"), "…which is a different question");
    assert!(!index.names_an_accessor("java", "compute"));
}

// ── AC4: two ambiguities, both resolving to nothing ─────────────────────────

#[test]
fn a_colliding_simple_name_in_one_module_resolves_to_nothing() {
    let mut index = PropertiesIndex::for_plugins(&[plugin("java")]);
    for (file, prefix) in [("m/a/C.java", "one"), ("m/b/C.java", "two")] {
        let body = format!(
            "@ConfigurationProperties(prefix = \"{prefix}\")\npublic class C {{ private String x; }}"
        );
        index.absorb_source(plugin("java"), file, "m", &body);
    }
    index.seal();
    assert!(
        index.get("C", "m").is_none(),
        "two declarations of one simple name inside ONE module prove neither",
    );
    assert!(index.collisions.contains("C"), "and the name is recorded as a collision");
    assert!(
        index.get("C", "elsewhere").is_none(),
        "the workspace fallback refuses a collided name too — a use site outside \
         the declaring module has even less to go on",
    );
}

#[test]
fn identical_declarations_in_several_modules_are_not_a_collision() {
    const PROPS: &str = r#"@ConfigurationProperties(prefix = "api")
                           public class C { private String host; }"#;
    let mut index = PropertiesIndex::for_plugins(&[plugin("java")]);
    for module in ["a", "b"] {
        index.absorb_source(plugin("java"), &format!("{module}/C.java"), module, PROPS);
    }
    index.seal();
    assert!(index.collisions.is_empty());
    assert_eq!(index.get("C", "a").expect("indexed").prefix, "api");
}

#[test]
#[cfg(feature = "lang-kotlin")]
fn an_accessor_matching_two_declared_properties_resolves_to_nothing() {
    // Reachable only where a language reads properties BOTH directly and through
    // a getter — Kotlin's `["", "get", "is"]`. `getUrl` then names `getUrl`
    // (direct) and `url` (stripped), and the class declares both.
    let index = index_one(
        "kt",
        "Opts.kt",
        "",
        r#"
        @ConfigurationProperties(prefix = "api")
        data class Opts(val url: String, val getUrl: String)
        "#,
    );
    let class = index.get("Opts", "").expect("indexed");
    assert_eq!(
        index.bind(class, "getUrl"),
        Err(BindingRefusal::AmbiguousProperty),
        "two candidate keys the class actually declares: neither is used",
    );
    assert_eq!(
        index.bind(class, "url").expect("binds").key,
        "api.url",
        "…while an unambiguous accessor on the same class still binds",
    );
}

#[test]
fn javas_vocabulary_cannot_produce_an_ambiguous_accessor() {
    // The pin behind the S-365 harness folding `AmbiguousProperty` into its own
    // `PropertyNotDeclared` census variant: under Java's shipped conventions the
    // refusal is unreachable, because no prefix is a prefix of another, so at
    // most one can ever strip a given name. Widen `accessor_prefixes` and this
    // fails, which is the point.
    let prefixes = &plugin("java")
        .semantics()
        .properties
        .as_ref()
        .expect("[properties]")
        .accessor_prefixes;
    for a in prefixes {
        for b in prefixes {
            assert!(
                a == b || !a.starts_with(b.as_str()),
                "{a:?} starts with {b:?}: two conventions could strip one name, \
                 so `AmbiguousProperty` becomes reachable for Java and the \
                 harness's refusal mapping in `resolve_getter` must widen with it",
            );
        }
    }
}

// ── AC2: a second language, as descriptor data ──────────────────────────────

#[test]
#[cfg(feature = "lang-kotlin")]
fn kotlin_binds_through_the_same_interpreter_with_no_core_edit() {
    // THE load-bearing case. Everything Kotlin about Kotlin is in
    // `plugins/kotlin/queries/properties.scm` and its `[properties]` table:
    // its annotation node carries no `name:` field, it has no
    // `element_value_pair`, and it has no `field_declaration`. The Java-shaped
    // walk this story deleted read all three by node kind and would index
    // nothing here. The code below is the SAME code path the Java cases above
    // drive.
    let index = index_one(
        "kt",
        "src/MailServerApi.kt",
        "mailbox",
        r#"
        @ConfigurationProperties(prefix = "mailserver.api")
        data class MailServerApi(val uriGetArchive: String, val timeout: Int)
        "#,
    );
    let class = index.get("MailServerApi", "mailbox").expect("indexed");
    assert_eq!(class.prefix, "mailserver.api");
    assert_eq!(
        class.properties,
        ["uriGetArchive", "timeout"]
            .into_iter()
            .map(canonical_key)
            .collect::<BTreeSet<_>>(),
    );
    // Both accessor conventions the Kotlin descriptor declares, on one class.
    assert_eq!(
        index.bind(class, "uriGetArchive").expect("direct access binds").key,
        "mailserver.api.uriGetArchive",
        "`\"\"` — Kotlin's own property syntax, a convention Java cannot express",
    );
    assert_eq!(
        index.bind(class, "getUriGetArchive").expect("the interop getter binds").key,
        "mailserver.api.uriGetArchive",
    );
}

#[test]
#[cfg(feature = "lang-kotlin")]
fn the_kotlin_descriptor_names_the_same_vocabulary_and_a_wider_convention() {
    let kotlin = plugin("kt");
    assert!(kotlin.capabilities().iter().any(|c| c == PROPERTIES_CAPABILITY));
    let descriptor = kotlin
        .semantics()
        .properties
        .as_ref()
        .expect("the [properties] descriptor section");
    assert_eq!(
        descriptor.annotations,
        ["ConfigurationProperties"],
        "Kotlin's Spring binding is annotation-compatible with Java's — what \
         differs is everything structural, which is why it needed a query file",
    );
    assert_eq!(
        descriptor.accessor_prefixes,
        ["", "get", "is"],
        "…and an accessor convention Java cannot express: `\"\"` is direct \
         property access",
    );
}

#[test]
#[cfg(feature = "lang-kotlin")]
fn a_kotlin_body_property_and_the_value_form_bind_too() {
    let index = index_one(
        "kt",
        "Other.kt",
        "",
        r#"
        @ConfigurationProperties("other.api")
        class Other {
            var baseUrl: String = ""
            val port: Int = 0
        }
        "#,
    );
    let class = index.get("Other", "").expect("indexed");
    assert_eq!(class.prefix, "other.api");
    assert_eq!(index.bind(class, "getBaseUrl").expect("binds").key, "other.api.baseUrl");
    assert_eq!(index.bind(class, "port").expect("binds").key, "other.api.port");
    assert_eq!(
        index.bind(class, "isEnabled"),
        Err(BindingRefusal::PropertyNotDeclared),
        "Kotlin's `is` row applies — the name strips to `enabled`, which this \
         class does not declare; it is not refused as a non-accessor",
    );
}

#[test]
#[cfg(feature = "lang-kotlin")]
fn one_index_spans_both_languages() {
    // The mixed-corpus posture, stated as behaviour rather than as prose: one
    // index over both plugins judges accessors under the UNION of their
    // conventions, which only ever widens the candidate set.
    let binders = [plugin("java"), plugin("kt")];
    let mut index = PropertiesIndex::for_plugins(&binders);
    index.absorb_source(
        plugin("java"),
        "a/J.java",
        "a",
        r#"@ConfigurationProperties(prefix = "j")
           public class J { private String host; }"#,
    );
    index.absorb_source(
        plugin("kt"),
        "b/K.kt",
        "b",
        "@ConfigurationProperties(prefix = \"k\")\ndata class K(val host: String)",
    );
    index.seal();
    assert_eq!(index.len(), 2);
    let j = index.get("J", "a").expect("java class");
    let k = index.get("K", "b").expect("kotlin class");
    assert_eq!(index.bind(j, "getHost").expect("binds").key, "j.host");
    assert_eq!(index.bind(k, "host").expect("binds").key, "k.host");
    // Each declaration is judged under ITS OWN language's convention. Kotlin's
    // direct-property-access row does not reach the Java class, and Java's
    // bean-getter rows do not make a Kotlin property read refuse.
    assert_eq!(
        index.bind(j, "host"),
        Err(BindingRefusal::NotAnAccessor),
        "Kotlin's empty prefix must not leak into a Java declaration",
    );
    assert_eq!(index.bind(k, "getHost").expect("kotlin also exposes the getter").key, "k.host");
}

/// The regression guard for the defect this design replaced: a flat UNION of
/// every declared convention made the shape predicate vacuously true, because
/// one language's empty prefix ("the name already is the property") answers yes
/// for every name in every language. `BindingRefusal::NotAnAccessor` — and with
/// it the harness census's `not a getter` row — silently emptied into its
/// neighbours in every index built over a default registry.
///
/// Asserted through `build`, the constructor the product actually uses, because
/// that is the one the earlier guard could not see: it inspected the Java
/// descriptor's own rows rather than the conventions the index judges by.
#[test]
#[cfg(feature = "lang-kotlin")]
fn a_second_languages_convention_does_not_leak_into_anothers_shape_test() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::write(root.join("pom.xml"), "<project/>\n").expect("write");
    std::fs::write(
        root.join("J.java"),
        "@ConfigurationProperties(prefix = \"j\")\npublic class J { private String url; private String getUrl; }",
    )
    .expect("write");

    let corpus = crate::extract::config::corpus::ConfigCorpus::discover(root);
    let index = PropertiesIndex::build(root, &corpus, registry());
    assert!(
        registry().iter().any(|p| p.name() == "kotlin"),
        "the guard is only meaningful while a linked plugin declares the empty prefix",
    );

    assert!(
        !index.names_an_accessor("java", "compute"),
        "a plain method call is not an accessor in Java, whatever other \
         languages the registry loaded declare",
    );
    let class = index.get("J", "").expect("indexed");
    assert_eq!(
        index.bind(class, "compute"),
        Err(BindingRefusal::NotAnAccessor),
        "…and the refusal survives all the way through `bind`",
    );
    // The other half of the same leak: under the union, `getUrl` on a class
    // declaring BOTH `url` and `getUrl` matched two candidates and refused as
    // ambiguous — for a language whose own vocabulary cannot produce ambiguity.
    assert_eq!(
        index.bind(class, "getUrl").expect("binds").key,
        "j.url",
        "Java's two prefixes cannot both strip one name, so this is unambiguous",
    );
}

// ── `build`: the corpus roster, the registry's admission rule, the pre-filter ─

#[test]
#[cfg(feature = "lang-kotlin")]
fn build_indexes_every_binding_language_the_registry_loaded() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let write = |rel: &str, body: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    };
    write("pom.xml", "<project/>\n");
    write(
        "src/J.java",
        "@ConfigurationProperties(prefix = \"j\")\npublic class J { private String host; }",
    );
    write("src/K.kt", "@ConfigurationProperties(prefix = \"k\")\ndata class K(val host: String)");
    write("src/Plain.java", "public class Plain { private String host; }");
    // Two near misses one character from admission, and one that IS admitted:
    // the extension test is the registry's, so `.java.txt` is a text file and
    // `.JAVA` is Java.
    write(
        "notes/J.java.txt",
        "@ConfigurationProperties(prefix = \"t\")\npublic class T { private String host; }",
    );
    write(
        "src/U.JAVA",
        "@ConfigurationProperties(prefix = \"u\")\npublic class U { private String host; }",
    );
    // Files whose plugin declares no binding vocabulary at all. They are in the
    // roster — it now carries every walked file — so `build` must step over them
    // rather than parse them. Review mutation-proved that branch unreached.
    write("src/main/resources/application.yml", "a:\n  b: 1\n");
    write("src/lib.rs", "// @ConfigurationProperties(prefix = \"r\")\npub struct R;\n");

    let corpus = crate::extract::config::corpus::ConfigCorpus::discover(root);
    let index = PropertiesIndex::build(root, &corpus, registry());

    assert_eq!(index.get("J", "").expect("java class").prefix, "j");
    assert_eq!(
        index.get("K", "").expect("kotlin class").prefix,
        "k",
        "one build indexes every language the registry loaded that declares the \
         capability — no roster is held in core",
    );
    assert_eq!(
        index.get("U", "").expect("an upper-cased extension is still Java").prefix,
        "u",
    );
    assert!(index.get("T", "").is_none(), "`.java.txt` is a text file, not Java");
    assert!(index.get("Plain", "").is_none(), "…and an unannotated class binds nothing");
    assert!(
        index.get("R", "").is_none(),
        "a language declaring no `[properties]` vocabulary contributes nothing, \
         even from a file whose text mentions the annotation",
    );
    assert_eq!(index.len(), 3);
}

// ── The structural guard behind AC2 ─────────────────────────────────────────

/// Node kinds the interpreter legitimately names that also exist in a JVM
/// grammar. Measured: this is the complete intersection today, and it is empty —
/// the interpreter names no node kind at all, because it reads only capture
/// names. An entry added here is a decision someone has to write down.
#[cfg(feature = "lang-kotlin")]
const BINDING_KIND_ALLOWLIST: &[&str] = &[];

/// The [NFR-MA-01] criterion proved structurally rather than by reading the
/// diff: the interpreter names **no** JVM grammar node kind, so there is nowhere
/// for a Java-shaped (or Kotlin-shaped) reading to hide in it.
///
/// Derived from the loaded grammars rather than from a hand-written list, for
/// the reason the sibling guard in `resolve::framework::tests::jvm_parity`
/// records: a closed list cannot notice what it does not name, and a complete
/// Kotlin-only walk over `class_declaration` → `modifiers` → `annotation` passed
/// the first version of that test because none of the three was listed.
///
/// [NFR-MA-01]: ../../../../docs/specs/requirements/NFR-MA-01.md
#[test]
#[cfg(feature = "lang-kotlin")]
fn the_interpreter_names_no_jvm_grammar_node_kind() {
    let mut jvm_kinds: BTreeSet<String> = BTreeSet::new();
    for ext in ["java", "kt"] {
        let language = plugin(ext).language();
        for id in 0..language.node_kind_count() {
            let Some(kind) = language.node_kind_for_id(id as u16) else {
                continue;
            };
            if language.node_kind_is_named(id as u16) {
                jvm_kinds.insert(kind.to_string());
            }
        }
    }
    assert!(
        jvm_kinds.contains("class_declaration") && jvm_kinds.contains("element_value_pair"),
        "the derived kind set must really come from the JVM grammars",
    );

    // RAW source, no comment stripping — the sibling guard's hard-won rule.
    // `resolve::framework::tests::jvm_parity::resolver_sources` records that
    // splitting each line at the first `//` was defeated twice in review:
    // `"http://value_argument"` named a guarded kind and passed because the cut
    // landed inside a string literal, and a hand-written scanner then mis-lexed
    // Rust's `'"'` char literal and stopped seeing code at all. Both failures
    // come from deciding what is a comment without knowing what is a string.
    //
    // Raw source needs no such decision, because this guard reads only
    // DOUBLE-QUOTED identifiers and `binding.rs` writes every node kind it
    // discusses in `backticks`. If a future comment does double-quote one, the
    // guard flags it and the fix is to backtick it like its neighbours.
    let code = include_str!("binding.rs");
    for literal in quoted_identifiers(code) {
        if !jvm_kinds.contains(literal.as_str()) {
            continue;
        }
        assert!(
            BINDING_KIND_ALLOWLIST.contains(&literal.as_str()),
            "binding.rs names the JVM node kind {literal:?}; the interpreter must \
             stay language-neutral and read capture names only. If this really is \
             unavoidable, add it to BINDING_KIND_ALLOWLIST with a reason.",
        );
    }
    // A language-specific reading need not name a node kind at all — a
    // `plugin.name() == "java"` branch would do — so that is asserted directly.
    // Every needle is DOUBLE-QUOTED, because a double-quoted literal is the only
    // way code can name one of these; a bare needle would trip on prose and is
    // what forced the comment-stripping the paragraph above retired.
    for id in [
        "\"java\"",
        "\"kotlin\"",
        "\"kt\"",
        "\"scala\"",
        "\"ConfigurationProperties\"",
    ] {
        assert!(
            !code.contains(id),
            "binding.rs names {id}; the interpreter must be driven by capture \
             names and descriptor data, never by which language it is looking at",
        );
    }
}

/// Every `"identifier"` string literal in `code` — the same simple scan the
/// sibling guard uses. Duplicated rather than shared: the two live in different
/// modules of different subsystems, and a shared test helper between them would
/// be a coupling neither wants.
#[cfg(feature = "lang-kotlin")]
fn quoted_identifiers(code: &str) -> Vec<String> {
    let bytes = code.as_bytes();
    let mut found = Vec::new();
    for (at, _) in code.match_indices('"') {
        let start = at + 1;
        let mut end = start;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
            end += 1;
        }
        if end > start && !bytes[start].is_ascii_digit() && bytes.get(end) == Some(&b'"') {
            found.push(code[start..end].to_string());
        }
    }
    found
}
