//! Unit tests for the use-site half of configuration binding (S-397).
//!
//! In their own file, like `binding_tests.rs`/`corpus_tests.rs` beside them.
//!
//! The population these cover is the one the invocation arm hands over: a
//! **captured operand node**, and the question "does the source prove which
//! configuration key this names?". Every case below that resolves has a twin
//! one hop away from resolving, because the refusals are what [FR-WS-19] AC3
//! is about and a positive-only suite proves nothing about them.

use super::*;

use crate::extract::config::binding::MEMBER_SCOPE;
use crate::plugin::{LanguagePlugin, LanguageRegistry};

/// The loaded registry, built once per test binary.
fn registry() -> &'static LanguageRegistry {
    static ONCE: std::sync::OnceLock<LanguageRegistry> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| LanguageRegistry::load(std::env::temp_dir()).expect("registry loads"))
}

fn java() -> &'static dyn LanguagePlugin {
    registry().for_extension("java").expect("java plugin")
}

/// A sealed index over one properties source — the declaration half the use
/// site is resolved against.
fn index(sources: &[(&str, &str)]) -> PropertiesIndex {
    let mut index = PropertiesIndex::for_plugins(&[java()]);
    for (rel, body) in sources {
        index.absorb_source(java(), rel, "", body);
    }
    index.seal();
    index
}

/// Parse `source` and return its tree, kept alive by the caller.
fn parse(source: &str) -> tree_sitter::Tree {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(java().language()).expect("set language");
    parser.parse(source, None).expect("parses")
}

/// The node whose source text is exactly `needle` — how a fixture names the
/// operand the invocation arm would have captured, without re-deriving the
/// arm's own capture here.
fn node_with_text<'t>(root: tree_sitter::Node<'t>, src: &[u8], needle: &str) -> tree_sitter::Node<'t> {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.utf8_text(src).is_ok_and(|t| t == needle) {
            return node;
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    panic!("no node spells {needle:?}");
}

/// Resolve `operand` (named by its source text) in `use_site`, against `index`,
/// returning the `${…}` placeholder the arm would record.
fn key_of(index: &PropertiesIndex, use_site: &str, operand: &str) -> Option<String> {
    let tree = parse(use_site);
    let src = use_site.as_bytes();
    let view = BindingView {
        index,
        types: DeclaredTypes::build(tree.root_node(), src),
        language: java().name(),
        module: "",
    };
    view.placeholder_for(node_with_text(tree.root_node(), src, operand), src)
}

/// The properties class every positive case below reads through.
const PROPS: &str = "@ConfigurationProperties(prefix = \"mailserver.api\")\n\
     public class MailServerConfigurationApi { private String uriGetArchive; }";

// ── DeclaredTypes: what a file proves about a name ──────────────────────────

/// A field declaration field-names its type on the declaration and its name on
/// the declarator, and a parameter carries both on itself. Both must read, or
/// the estate's dominant shape — a constructor-injected `private final` field —
/// resolves to nothing.
#[test]
fn a_field_and_a_parameter_each_yield_their_declared_simple_type() {
    let source = "package a;\n\
        public class Caller {\n\
          private final MailServerConfigurationApi api;\n\
          Caller(com.acme.props.OtherProps<String> other) { }\n\
          void go() { LocalProps local = null; }\n\
        }";
    let tree = parse(source);
    let types = DeclaredTypes::build(tree.root_node(), source.as_bytes());

    assert_eq!(types.get("api"), Some("MailServerConfigurationApi"), "a field");
    assert_eq!(
        types.get("other"),
        Some("OtherProps"),
        "a parameter, with its package qualification and its generic arguments \
         reduced to the simple name a use site spells",
    );
    assert_eq!(types.get("local"), Some("LocalProps"), "a local variable");
}

/// A method declares a name and a return type, and binding the two would
/// register `getUriGetArchive` as a value of type `String`. The callable-field
/// skip is what stops it, and a class body's own name is skipped the same way.
#[test]
fn a_callable_declaration_binds_no_name_to_its_return_type() {
    let source = "public class Caller { String getUriGetArchive() { return null; } }";
    let tree = parse(source);
    let types = DeclaredTypes::build(tree.root_node(), source.as_bytes());

    assert_eq!(types.get("getUriGetArchive"), None, "a method is not a value binding");
    assert_eq!(types.get("Caller"), None, "…nor is the class it is declared in");
}

/// Two disagreeing declarations of one name resolve to nothing: this walk is
/// scope-blind, so choosing either would be a guess ([NFR-RA-05]). Two
/// *agreeing* ones — the idiomatic constructor-injection pair — still resolve.
#[test]
fn a_name_declared_under_two_types_resolves_to_nothing_and_an_agreeing_pair_still_does() {
    let disagreeing = "public class Caller {\n\
          private final MailServerConfigurationApi api;\n\
          void other(SomethingElse api) { }\n\
        }";
    let tree = parse(disagreeing);
    assert_eq!(
        DeclaredTypes::build(tree.root_node(), disagreeing.as_bytes()).get("api"),
        None,
        "the file declares `api` twice under different types and proves neither",
    );

    let agreeing = "public class Caller {\n\
          private final MailServerConfigurationApi api;\n\
          Caller(MailServerConfigurationApi api) { this.api = api; }\n\
        }";
    let tree = parse(agreeing);
    assert_eq!(
        DeclaredTypes::build(tree.root_node(), agreeing.as_bytes()).get("api"),
        Some("MailServerConfigurationApi"),
        "constructor injection declares the same name twice with ONE type",
    );
}

// ── The chain: accessor → field → owning class → prefix → canonical key ─────

/// The positive case, end to end, and the key is **canonical**: the source
/// spells `getUriGetArchive` and a yaml spells `uri-get-archive`, and both
/// canonicalise to the one form resolution matches on.
#[test]
fn an_accessor_on_an_injected_properties_class_names_its_canonical_key() {
    let index = index(&[("Props.java", PROPS)]);
    let use_site = "public class Caller {\n\
          private final MailServerConfigurationApi api;\n\
          void go() { client.get(api.getUriGetArchive()); }\n\
        }";
    assert_eq!(
        key_of(&index, use_site, "api.getUriGetArchive()").as_deref(),
        Some("${mailserver.api.urigetarchive}"),
    );
}

/// Four refusals, each one hop from the case above, each resolving to
/// **nothing** rather than to a guess ([FR-WS-19] AC3, [NFR-RA-05]).
#[test]
fn every_unproven_hop_resolves_to_nothing() {
    let index = index(&[("Props.java", PROPS)]);
    let header = "public class Caller {\n  private final MailServerConfigurationApi api;\n";

    for (operand, body, why) in [
        (
            "api.compute()",
            "  void go() { client.get(api.compute()); }\n",
            "not an accessor under Java's `get`/`is` convention",
        ),
        (
            "api.getMissing()",
            "  void go() { client.get(api.getMissing()); }\n",
            "an accessor naming a property the class does not declare",
        ),
        (
            "stranger.getUriGetArchive()",
            "  void go() { client.get(stranger.getUriGetArchive()); }\n",
            "a receiver whose declared type this file never states",
        ),
        (
            "api.getNested().getUriGetArchive()",
            "  void go() { client.get(api.getNested().getUriGetArchive()); }\n",
            "a NESTED accessor — the receiver is a whole call expression, which \
             no file declares a type for, so the nested-properties ceiling each \
             `properties.scm` records is enforced by the same hop that refuses \
             any other unknown receiver",
        ),
    ] {
        let use_site = format!("{header}{body}}}");
        assert_eq!(key_of(&index, &use_site, operand), None, "{operand}: {why}");
    }
}

/// A `System.getenv` read names no configuration key, so it never reaches the
/// committed corpus at all — the refusal [FR-WS-19] AC4 requires, held here at
/// the hop this story adds rather than only at the resolution it feeds.
///
/// The reason is worth naming exactly, because the obvious one is wrong:
/// `getenv` **does** pass Java's accessor-shape test (strip `get`, yielding
/// `env`). What refuses it is the receiver — `System` is a type, and this file
/// declares no name of that spelling — so the read never reaches a properties
/// class to bind against.
#[test]
fn an_environment_read_names_no_configuration_key() {
    let index = index(&[("Props.java", PROPS)]);
    let use_site = "public class Caller {\n\
          void go() { client.get(System.getenv(\"BASE_URL\")); }\n\
        }";
    assert_eq!(key_of(&index, use_site, "System.getenv(\"BASE_URL\")"), None);
}

/// A simple name two disagreeing declarations claim is a
/// [`PropertiesIndex`] collision, and the use site sees **nothing** — the
/// second half of [FR-WS-19] AC3, asserted through this module because that is
/// where a use site meets it.
#[test]
fn a_colliding_properties_class_leaves_the_use_site_with_no_key() {
    let index = index(&[
        ("a/Props.java", PROPS),
        (
            "b/Props.java",
            "@ConfigurationProperties(prefix = \"other.api\")\n\
             public class MailServerConfigurationApi { private String uriGetArchive; }",
        ),
    ]);
    let use_site = "public class Caller {\n\
          private final MailServerConfigurationApi api;\n\
          void go() { client.get(api.getUriGetArchive()); }\n\
        }";
    assert_eq!(key_of(&index, use_site, "api.getUriGetArchive()"), None);
}

/// A receiver that is not a plain identifier is refused rather than trimmed to
/// its last segment: `holder.api` and a local `api` are different objects, and
/// the trim would resolve one against the other.
#[test]
fn a_qualified_receiver_is_refused_rather_than_trimmed() {
    let index = index(&[("Props.java", PROPS)]);
    let use_site = "public class Caller {\n\
          private final Holder holder;\n\
          private final MailServerConfigurationApi api;\n\
          void go() { client.get(holder.api.getUriGetArchive()); }\n\
        }";
    assert_eq!(key_of(&index, use_site, "holder.api.getUriGetArchive()"), None);
}

/// **The convention leak, at the use site.** A Java file reading a
/// Kotlin-declared properties class is judged by **Java's** accessor
/// convention, not by the declaring class's.
///
/// This is what the shape test in [`BindingView::key_for`] is for, and the only
/// case in which it changes an answer. Kotlin declares the empty accessor
/// prefix — direct property access — under which *every* name is already a
/// property name; `bind` judges by the declaring class's language, so without
/// the reading language's own shape test a Java `k.compute()` would bind to the
/// Kotlin `compute` property and name a key the Java source cannot read.
///
/// The sibling half of `binding_tests`'
/// `a_second_languages_convention_does_not_leak_into_anothers_shape_test`,
/// which pins the same leak inside the index.
#[test]
#[cfg(feature = "lang-kotlin")]
fn a_java_use_site_is_judged_by_javas_convention_not_the_declaring_languages() {
    let kotlin = registry().for_extension("kt").expect("kotlin plugin");
    let mut index = PropertiesIndex::for_plugins(&[java(), kotlin]);
    index.absorb_source(
        kotlin,
        "K.kt",
        "",
        "@ConfigurationProperties(prefix = \"k\")\ndata class K(val compute: String)",
    );
    index.seal();
    assert!(index.get("K", "").is_some(), "the Kotlin class is indexed");

    let use_site = "public class Caller {\n\
          private final K k;\n\
          void go() { client.get(k.compute()); }\n\
        }";
    assert_eq!(
        key_of(&index, use_site, "k.compute()"),
        None,
        "`compute` is not an accessor in Java, whatever the declaring language \
         admits",
    );
    // …and the same class DOES bind through a name Java's convention reads, so
    // the refusal above is about the convention and not about the fixture being
    // unresolvable in the first place.
    assert_eq!(
        key_of(&index, &use_site.replace("k.compute()", "k.getCompute()"), "k.getCompute()")
            .as_deref(),
        Some("${k.compute}"),
    );
}

// ── The two fabrications review reproduced, and the one that stays ──────────

/// **A node that CALLS is not a node that declares.** A Java `method_invocation`
/// carries `name`, `object` and `arguments` and no `parameters` or `body`, so
/// before `arguments` joined [`CALLABLE_FIELDS`] the parent hop read the `type`
/// of an enclosing `cast_expression` and registered the *called method's* name
/// against it.
///
/// The end-to-end harm is in `extract::tests` — a cast anywhere in the file gave
/// an undeclared receiver a type and turned a refusal into a `config-bound`
/// target. Pinned here at the unit that produced it, and with the two
/// non-fabricating shapes beside it so a future widening has to face all three.
#[test]
fn a_call_expression_binds_no_name_however_its_result_is_cast() {
    let source = "public class Caller {\n\
          void warm(Registry reg) {\n\
            Object o = (MailServerConfigurationApi) reg.lookup();\n\
            MailServerConfigurationApi declared = reg.other();\n\
          }\n\
        }";
    let tree = parse(source);
    let types = DeclaredTypes::build(tree.root_node(), source.as_bytes());

    assert_eq!(
        types.get("lookup"),
        None,
        "the CALLED method's name is not a value of the cast type",
    );
    assert_eq!(
        types.get("other"),
        None,
        "…nor of the type its result is assigned to",
    );
    assert_eq!(
        types.get("o"),
        Some("Object"),
        "the declarator the cast is assigned to still binds, so the fix narrowed \
         the fabrication and not the capture",
    );
    assert_eq!(types.get("declared"), Some("MailServerConfigurationApi"));
}

/// The **stated ceiling** the same walk still carries, pinned so it is a known
/// quantity rather than a surprise: a Java annotation-type element is
/// field-identical to a parameter, so it binds.
///
/// This asserts the ceiling, not an intention — see the module docs for why no
/// field separates the two and what bounds the reach. The second half is the
/// bound that matters in practice: a real declaration of the same name under a
/// different type poisons the entry rather than losing to it.
#[test]
fn an_annotation_type_element_still_binds_and_a_real_declaration_poisons_it() {
    let alone = "public class Caller {\n\
          @interface Marker { MailServerConfigurationApi api(); }\n\
        }";
    let tree = parse(alone);
    assert_eq!(
        DeclaredTypes::build(tree.root_node(), alone.as_bytes()).get("api"),
        Some("MailServerConfigurationApi"),
        "the stated ceiling: no grammar field tells an annotation element from a \
         parameter, so it binds",
    );

    let beside = "public class Caller {\n\
          @interface Marker { MailServerConfigurationApi api(); }\n\
          private final SomethingElse api;\n\
        }";
    let tree = parse(beside);
    assert_eq!(
        DeclaredTypes::build(tree.root_node(), beside.as_bytes()).get("api"),
        None,
        "…and a real, disagreeing declaration of that name poisons it to nothing",
    );
}

/// **A key the placeholder reader would not read back is not proven.**
///
/// `canonical_key` lowercases and drops `-`/`_`; every other byte of the
/// annotation's prefix literal survives into the string the arm records. Two of
/// them break the reader: `}` ends the placeholder early, and `:` starts an
/// inline default. Both truncate to a **shorter key a corpus can define**, so
/// without the round-trip the site binds a value it never named.
///
/// The near misses are one character from the admitted case, which is the only
/// way to probe a matcher.
#[test]
fn a_key_the_placeholder_reader_cannot_read_back_resolves_to_nothing() {
    let use_site = "public class Caller {\n\
          private final MailServerConfigurationApi api;\n\
          void go() { client.get(api.getUriGetArchive()); }\n\
        }";
    let props = |prefix: &str| {
        format!(
            "@ConfigurationProperties(prefix = \"{prefix}\")\n\
             public class MailServerConfigurationApi {{ private String uriGetArchive; }}"
        )
    };

    for (prefix, why) in [
        ("mail:server.api", "a colon reads as the inline-default separator"),
        ("mail}server.api", "a closing brace ends the placeholder early"),
        ("mail${x}.api", "a nested placeholder is not one key"),
    ] {
        let index = index(&[("P.java", &props(prefix))]);
        assert_eq!(
            key_of(&index, use_site, "api.getUriGetArchive()"),
            None,
            "prefix {prefix:?}: {why}",
        );
    }

    // …and the admitted case, so the guard is proved to be narrow rather than
    // total. A dot and a digit are the characters a real prefix is made of.
    let index = index(&[("P.java", &props("mail2server.api"))]);
    assert_eq!(
        key_of(&index, use_site, "api.getUriGetArchive()").as_deref(),
        Some("${mail2server.api.urigetarchive}"),
    );
}

/// [`MEMBER_SCOPE`] is pinned to the **literal the coverage tier reads with**,
/// not merely to itself.
///
/// `federation::coverage::record_config_bound` builds its resolver as
/// `Resolver { corpus, module: "" }`. If the index absorbed under any other
/// scope, a class would be looked up in one scope while its values were read in
/// another — and every test that spells the scope as `MEMBER_SCOPE` on both
/// sides would still pass, which is exactly what a mutation to `"member"`
/// demonstrated.
#[test]
fn the_member_scope_is_the_empty_scope_the_coverage_tier_reads_with() {
    assert_eq!(
        crate::extract::config::binding::MEMBER_SCOPE,
        "",
        "the coverage tier hardcodes `Resolver {{ module: \"\" }}`; these two \
         literals are one agreement and must be changed together",
    );

    // The behavioural half: an index built the way ingestion builds it answers a
    // lookup made the way the coverage tier makes it.
    let mut index = PropertiesIndex::for_plugins(&[java()]);
    index.absorb_source(java(), "P.java", MEMBER_SCOPE, PROPS);
    index.seal();
    assert!(index.get("MailServerConfigurationApi", "").is_some());
}

/// **The Kotlin use-site ceiling, pinned.** A Kotlin file's member call carries
/// no `object`/`name` fields at all, so [`member_call`] never recognises it and
/// a Kotlin use site resolves to nothing.
///
/// Asserted so the ceiling is a measured fact rather than an assumption: Kotlin
/// is one of the two languages shipping the `properties` capability, and the
/// declaring half works for it (`binding_tests`'
/// `kotlin_binds_through_the_same_interpreter_with_no_core_edit`) while the
/// reading half does not. The second assertion is the control — the *same*
/// class, read from Java, does resolve — so this pins the grammar's spelling
/// and not a broken fixture.
#[test]
#[cfg(feature = "lang-kotlin")]
fn a_kotlin_use_site_is_not_reached_and_the_same_class_still_resolves_from_java() {
    let kotlin = registry().for_extension("kt").expect("kotlin plugin");
    let mut index = PropertiesIndex::for_plugins(&[java(), kotlin]);
    index.absorb_source(
        kotlin,
        "K.kt",
        MEMBER_SCOPE,
        "@ConfigurationProperties(prefix = \"k\")\ndata class K(val host: String)",
    );
    index.seal();

    let source = "class Caller(private val k: K) {\n\
          fun go() { client.get(k.host) }\n\
        }";
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(kotlin.language()).expect("set language");
    let tree = parser.parse(source, None).expect("parses");
    let src = source.as_bytes();
    let view = BindingView {
        index: &index,
        types: DeclaredTypes::build(tree.root_node(), src),
        language: kotlin.name(),
        module: MEMBER_SCOPE,
    };
    assert_eq!(
        view.placeholder_for(node_with_text(tree.root_node(), src, "k.host"), src),
        None,
        "Kotlin spells a member read positionally, with no `object`/`name` \
         fields, so the use-site half does not reach it",
    );

    // The control: the very same declaration resolves from a Java use site, so
    // the refusal above is the Kotlin GRAMMAR's spelling and not a dead index.
    let java_site = "public class Caller {\n\
          private final K k;\n\
          void go() { client.get(k.getHost()); }\n\
        }";
    assert_eq!(
        key_of(&index, java_site, "k.getHost()").as_deref(),
        Some("${k.host}"),
    );
}

/// **The shape every fixture in this task actually has**: a member that declares
/// a bound class and commits **no** configuration source.
///
/// The hop changes what such a member reports, and the change is worth pinning
/// because it is easy to read as a regression. Before, the accessor site was a
/// keyless row the coverage tier calls `base-url-runtime` — "the path is
/// composed at runtime", which was false about the repository. Now it is a
/// config-bound target whose key no committed source defines, which resolves to
/// [`ValueRefusal::MissingKey`] and reports as `config-key-missing` — naming the
/// key that could not be proved, which is what [FR-WS-19] AC3's first clause
/// asks for.
///
/// Still a refusal either way; what moved is which one, and that shows up in the
/// census [S-397](../../../../docs/planning/journal.md) T2 measures.
///
/// [FR-WS-19]: ../../../../docs/specs/requirements/FR-WS-19.md
#[test]
fn a_bound_class_with_no_committed_source_refuses_by_naming_the_missing_key() {
    use crate::graph_store::ConfigDefinition;
    use crate::resolve::binding::{ConfigLookup, Resolver, ValueRefusal};

    /// A member whose configuration corpus is empty — every key is undefined.
    struct NoCorpus;
    impl ConfigLookup for NoCorpus {
        fn definitions(&self, _key: &str, _module: &str) -> Vec<ConfigDefinition> {
            Vec::new()
        }
    }

    let index = index(&[("Props.java", PROPS)]);
    let use_site = "public class Caller {\n\
          private final MailServerConfigurationApi api;\n\
          void go() { client.get(api.getUriGetArchive()); }\n\
        }";
    let recorded =
        key_of(&index, use_site, "api.getUriGetArchive()").expect("the accessor resolves a key");

    // WHICH key is refused is half the claim, and over an empty corpus the
    // refusal alone cannot show it — every key is missing there, so a fabricated
    // one would refuse identically. Assert the identity through the reader the
    // coverage tier uses, so the row names the accessor's own key.
    assert_eq!(
        crate::resolve::binding::placeholder_keys(&recorded),
        Some(vec!["mailserver.api.urigetarchive".to_string()]),
        "the recorded target names the accessor's canonical key, and only it",
    );

    // The coverage tier's own reader, built the way it builds it.
    let resolver = Resolver { corpus: &NoCorpus, module: MEMBER_SCOPE };
    assert_eq!(
        resolver.resolve_template(&recorded).expect("the target carries a placeholder"),
        Err(ValueRefusal::MissingKey),
        "…and that key is unproved, so the site refuses rather than binding",
    );
}
