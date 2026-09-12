//! Unit tests for the committed-configuration corpus (S-380).
//!
//! In their own file, like `tests.rs`/`proto_tests.rs`/`graphql_tests.rs` beside
//! them. The first 22 moved here verbatim with the code they cover, out of the
//! S-365 measurement harness — AC2 turns on their being unmodified, so a change
//! to one of them is a claim that the flattener's behaviour changed.

use super::*;

// ── Promoted verbatim from the S-365 measurement harness ────────────────
//
// These moved with the code they cover (AC2). Each one was written against a
// real mis-read on the reference estate, so a failure here is a claim that
// the flattener's behaviour changed, not that a fixture drifted.
#[test]
fn relaxed_binding_makes_the_three_spellings_one_key() {
    let canonical = canonical_key("mailserver.api.uri-get-archive");
    assert_eq!(canonical, canonical_key("mailserver.api.uriGetArchive"));
    assert_eq!(canonical, canonical_key("mailserver.api.URI_GET_ARCHIVE"));
    assert_eq!(canonical, "mailserver.api.urigetarchive");
}

#[test]
fn relaxed_binding_does_not_merge_distinct_segments() {
    assert_ne!(canonical_key("a.b.c"), canonical_key("a.bc"));
}

#[test]
fn yaml_flattens_nested_scalars_and_honours_document_separators() {
    let flat = parse_yaml(
        "mailserver:\n  api:\n    base-url: http://localhost:8000\n    \
         uri-get-archive: /x/{id}\n---\nother: 1\n",
    );
    assert_eq!(
        flat.get("mailserver.api.baseurl").map(|v| v.iter().next().unwrap().as_str()),
        Some("http://localhost:8000"),
    );
    assert_eq!(
        flat.get("mailserver.api.urigetarchive").map(|v| v.iter().next().unwrap().as_str()),
        Some("/x/{id}"),
    );
    // The separator reset the indent stack, so `other` is top-level.
    assert!(flat.contains_key("other"), "got {flat:?}");
}

#[test]
fn yaml_reads_a_sequence_valued_key_as_absent_rather_than_as_a_scalar() {
    // Under-reading is the safe direction: it can only lower the
    // newly-admitted count, never inflate it.
    let flat = parse_yaml("topics:\n  - a\n  - b\nplain: v\n");
    assert!(!flat.contains_key("topics"));
    assert!(flat.contains_key("plain"));
}

#[test]
fn yaml_skips_a_block_scalar_body_without_swallowing_the_next_key() {
    let flat = parse_yaml("banner: |\n  line one\n  line two\nnext: v\n");
    assert!(!flat.contains_key("banner"));
    assert_eq!(flat.get("next").map(|v| v.iter().next().unwrap().as_str()), Some("v"));
}

#[test]
fn a_blank_line_does_not_end_a_block_scalar_early() {
    let flat = parse_yaml("banner: |\n  one\n\n  key: not-a-key\nnext: v\n");
    assert!(!flat.contains_key("banner.key"), "got {flat:?}");
    assert!(!flat.contains_key("key"), "got {flat:?}");
    assert_eq!(flat.get("next").map(|v| v.iter().next().unwrap().as_str()), Some("v"));
}

#[test]
fn a_flow_collection_is_not_read_as_a_scalar_value() {
    // Mis-reading `topics: [a, b]` as the string "[a, b]" would let a
    // list-valued key be reported as agreed on a value it does not have.
    let flat = parse_yaml("topics: [a, b]\nmapping: {k: v}\nplain: p\n");
    assert!(!flat.contains_key("topics"), "got {flat:?}");
    assert!(!flat.contains_key("mapping"), "got {flat:?}");
    assert!(flat.contains_key("plain"));
}

#[test]
fn properties_read_both_separators_comments_and_continuations() {
    let flat = parse_properties(
        "# comment\n! also a comment\na.b=1\nc.d: 2\ne.f=one\\\ntwo\n",
    );
    assert_eq!(flat.get("a.b").map(|v| v.iter().next().unwrap().as_str()), Some("1"));
    assert_eq!(flat.get("c.d").map(|v| v.iter().next().unwrap().as_str()), Some("2"));
    assert_eq!(flat.get("e.f").map(|v| v.iter().next().unwrap().as_str()), Some("onetwo"));
}

#[test]
fn a_quoted_value_loses_its_quotes_and_an_unquoted_one_loses_its_trailing_comment() {
    let flat = parse_yaml("a: \"/x\"\nb: /y # why\nc: '/z'\n");
    for (key, want) in [("a", "/x"), ("b", "/y"), ("c", "/z")] {
        assert_eq!(
            flat.get(key).map(|v| v.iter().next().unwrap().as_str()),
            Some(want),
            "key {key}",
        );
    }
}

#[test]
fn a_line_whose_key_is_not_one_token_registers_no_key() {
    let flat = parse_yaml("real-key: v\nsome prose: with a colon\n");
    assert!(flat.contains_key("realkey"));
    assert_eq!(flat.len(), 1, "got {flat:?}");
}

#[test]
fn a_value_containing_a_colon_keeps_its_whole_value() {
    let flat = parse_yaml("url: http://host:8080/p\n");
    assert_eq!(
        flat.get("url").map(|v| v.iter().next().unwrap().as_str()),
        Some("http://host:8080/p"),
    );
}

// ── Regression fixtures: every defect the S-365 review found ───────────
//
// Each of these failed before its fix. They are grouped because they share
// a property: all were invisible to the 46 fixtures that preceded them, and
// most moved a published number.

#[test]
fn a_commented_mapping_header_is_not_read_as_a_scalar() {
    // Was: `api:   # the mail API` recorded `mailserver.api => "# the mail
    // API"` AND re-parented the whole subtree, so the real key vanished.
    let flat = parse_yaml(
        "mailserver:\n  api:   # the mail API\n    uri-get-archive: /a\n",
    );
    assert_eq!(
        flat.get("mailserver.api.urigetarchive").map(|v| v.iter().next().unwrap().as_str()),
        Some("/a"),
        "the subtree must stay under its header; got {flat:?}",
    );
    assert!(!flat.contains_key("mailserver.api"), "got {flat:?}");
}

#[test]
fn a_sequence_items_own_keys_do_not_become_the_parents() {
    // Was: `- name: primary` was skipped but the item's CONTINUATION lines
    // registered at the parent path, fabricating a real-looking key from a
    // list element.
    let flat = parse_yaml(
        "mailserver:\n  api:\n    - name: primary\n      uri-get-archive: /wrong\n",
    );
    assert!(!flat.contains_key("mailserver.api.urigetarchive"), "got {flat:?}");
    assert!(!flat.contains_key("mailserver.api.name"), "got {flat:?}");
}

#[test]
fn a_sequence_does_not_swallow_the_key_that_follows_it() {
    let flat = parse_yaml("list:\n  - a: 1\n    b: 2\nafter: v\n");
    assert_eq!(flat.get("after").map(|v| v.iter().next().unwrap().as_str()), Some("v"));
}

#[test]
fn a_quoted_value_with_a_trailing_comment_loses_both() {
    // Was: the quote test required the string to END with the quote, which
    // a trailing comment defeats, so the value kept its quotes — a false
    // disagreement against an unquoted definition elsewhere.
    let flat = parse_yaml("a: \"/x\" # why\nb: '/y' # why\n");
    assert_eq!(flat.get("a").map(|v| v.iter().next().unwrap().as_str()), Some("/x"));
    assert_eq!(flat.get("b").map(|v| v.iter().next().unwrap().as_str()), Some("/y"));
}

#[test]
fn a_value_may_contain_a_hash_that_is_not_a_comment() {
    let flat = parse_yaml("frag: \"/x#anchor\"\n");
    assert_eq!(flat.get("frag").map(|v| v.iter().next().unwrap().as_str()), Some("/x#anchor"));
}

#[test]
fn every_block_scalar_indicator_skips_its_body() {
    // Was: only the four bare spellings were caught, so `|+` and `|2` were
    // recorded as VALUES and their bodies parsed as YAML — a bogus agreed
    // value plus phantom keys lifted out of the block.
    for indicator in ["|", ">", "|-", ">-", "|+", ">+", "|2"] {
        let flat = parse_yaml(&format!("banner: {indicator}\n  one\n  key: v\nnext: n\n"));
        assert!(!flat.contains_key("banner"), "{indicator}: got {flat:?}");
        assert!(!flat.contains_key("key"), "{indicator}: leaked a block line: {flat:?}");
        assert_eq!(
            flat.get("next").map(|v| v.iter().next().unwrap().as_str()),
            Some("n"),
            "{indicator}: lost the key after the block",
        );
    }
}

#[test]
fn an_anchor_is_a_mapping_header_not_a_value() {
    let flat = parse_yaml("defaults: &d\n  url: /a\n");
    assert_eq!(
        flat.get("defaults.url").map(|v| v.iter().next().unwrap().as_str()),
        Some("/a"),
        "got {flat:?}",
    );
    assert!(!flat.contains_key("defaults"), "got {flat:?}");
}

#[test]
fn a_properties_value_is_not_put_through_the_yaml_rules() {
    // In a .properties file `#` is not an inline comment and quotes are
    // literal. Borrowing the YAML rules changed the value Java would read,
    // which can make two sources falsely agree.
    let flat = parse_properties("a=/x # main\nb=\"/y\"\n");
    assert_eq!(flat.get("a").map(|v| v.iter().next().unwrap().as_str()), Some("/x # main"));
    assert_eq!(flat.get("b").map(|v| v.iter().next().unwrap().as_str()), Some("\"/y\""));
}

#[test]
fn a_properties_escaped_backslash_is_a_value_not_a_continuation() {
    // Was: any trailing backslash continued the line, so `a=C:\\tmp\\`
    // swallowed the entry after it and `b` disappeared entirely.
    let flat = parse_properties("a=C:\\\\tmp\\\\\nb=2\n");
    assert!(flat.contains_key("b"), "the next entry was swallowed: {flat:?}");
    assert_eq!(flat.get("b").map(|v| v.iter().next().unwrap().as_str()), Some("2"));
}

#[test]
fn a_properties_odd_backslash_still_continues() {
    let flat = parse_properties("a=one\\\ntwo\n");
    assert_eq!(flat.get("a").map(|v| v.iter().next().unwrap().as_str()), Some("onetwo"));
}

#[test]
fn a_profile_variant_is_recognised_whole() {
    assert_eq!(config_profile("application.yml"), Some(None));
    assert_eq!(config_profile("application.yaml"), Some(None));
    assert_eq!(config_profile("application.properties"), Some(None));
    assert_eq!(
        config_profile("application-it-jenkins.yml"),
        Some(Some("it-jenkins".to_string())),
        "a hyphen inside the profile must not split it",
    );
    assert_eq!(config_profile("bootstrap.yml"), None);
    assert_eq!(config_profile("application.json"), None);
    assert_eq!(config_profile("application-.yml"), None);
}

// ── The ingestion entry point (S-380) ───────────────────────────────────

#[test]
fn a_source_flattens_to_its_profile_and_its_full_depth_keys() {
    // The purpose of the story in one assertion: `api.uri-get-mailbox` is a
    // depth-3 key, invisible to the depth-2 ConfigSection walk, and it must
    // arrive here carrying its VALUE.
    let facts = source_facts(
        "svc/src/main/resources/application-dev.yml",
        "mailserver:\n  api:\n    uri-get-mailbox: /mailbox/{id}\n",
    )
    .expect("an application-<profile>.yml is a configuration source");
    assert_eq!(facts.profile.as_deref(), Some("dev"));
    assert_eq!(
        facts.values,
        vec![ConfigValueFact {
            key: "mailserver.api.urigetmailbox".to_string(),
            value: "/mailbox/{id}".to_string(),
        }],
    );
}

#[test]
fn an_unprofiled_source_carries_no_profile_but_still_carries_its_values() {
    let facts = source_facts("application.yml", "a:\n  b: 1\n").expect("a source");
    assert_eq!(facts.profile, None);
    assert_eq!(facts.values.len(), 1, "got {:?}", facts.values);
}

#[test]
fn a_properties_source_is_parsed_by_the_properties_rules_not_the_yaml_ones() {
    // The extension picks the parser, exactly as `ConfigCorpus::discover`
    // picks it — so an inline `#` stays in the value here and would not in
    // a `.yml` file.
    let facts = source_facts("application.properties", "a.b=/x # main\n").expect("a source");
    assert_eq!(
        facts.values,
        vec![ConfigValueFact { key: "a.b".to_string(), value: "/x # main".to_string() }],
    );
}

#[test]
fn a_file_that_is_not_a_configuration_source_yields_nothing_at_all() {
    // What keeps a member with no configuration corpus byte-for-byte
    // unaffected: every one of these is a real file the artifact extraction
    // pass already routes through, and none of them may write a row.
    for path in [
        "docker-compose.yml",
        "svc/k8s/deployment.yaml",
        "bootstrap.yml",
        "application.json",
        "gradle.properties",
        "src/main/java/App.java",
    ] {
        assert!(
            source_facts(path, "a:\n  b: 1\n").is_none(),
            "{path} must not be read as a configuration source",
        );
    }
}

#[test]
fn a_key_defined_twice_across_documents_keeps_both_values() {
    // Multi-document YAML proves two values for one key; neither is dropped
    // and neither is averaged (FR-WS-19: disagreement is represented).
    let facts = source_facts("application.yml", "a: one\n---\na: two\n").expect("a source");
    assert_eq!(
        facts.values,
        vec![
            ConfigValueFact { key: "a".to_string(), value: "one".to_string() },
            ConfigValueFact { key: "a".to_string(), value: "two".to_string() },
        ],
    );
}

#[test]
fn config_profile_and_source_facts_agree_on_admission() {
    // The two entry points must apply the same RULE: whatever `config_profile`
    // admits for the walk, `source_facts` admits for ingestion, and with the
    // same profile.
    //
    // This is a statement about the rule, not about the populations. The
    // populations genuinely differ — `.properties` is admitted here and
    // unreachable in production, for the routing reason `source_facts`'s own
    // doc comment gives — so do not read a green run here as parity between
    // the census and the store.
    for name in [
        "application.yml",
        "application.yaml",
        "application.properties",
        "application-it-jenkins.yml",
        "bootstrap.yml",
        "application-.yml",
    ] {
        assert_eq!(
            source_facts(name, "a: 1\n").map(|f| f.profile),
            config_profile(name),
            "{name}: ingestion and discovery disagree about admission",
        );
    }
}

#[test]
fn only_the_basename_decides_admission_not_the_directory() {
    let deep = source_facts("a/b/c/d/application-prod.properties", "k=v\n");
    assert_eq!(deep.map(|f| f.profile), Some(Some("prod".to_string())));
}

// ── The discovery entry point (S-380) ───────────────────────────────────
//
// `discover`, `module_of`, `profiles` and `props_candidates` are production
// symbols whose only other coverage is the measurement harness, which skips
// unless `LOGOS_REF_WORKSPACE` names a private 84-repo estate. Without these
// two cases they are untested on CI and on any machine without that
// checkout, and read as green there. Both run in milliseconds over a
// tempdir and need no estate.

#[test]
fn discover_places_each_source_in_its_nearest_module_and_reads_its_profile() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let write = |rel: &str, body: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    };
    // Two nested module roots: the longest path-segment prefix must win, which
    // is the whole point of `module_of`.
    write("a/pom.xml", "<project/>\n");
    write("a/b/pom.xml", "<project/>\n");
    write("a/b/src/main/resources/application-dev.yml", "server:\n  port: 8080\n");
    write("a/src/main/resources/application.yml", "server:\n  port: 9090\n");

    let corpus = ConfigCorpus::discover(root);
    let placed: Vec<(&str, &str, Option<&str>)> = corpus
        .sources
        .iter()
        .map(|s| (s.path.as_str(), s.module.as_str(), s.profile.as_deref()))
        .collect();
    assert_eq!(
        placed,
        vec![
            ("a/b/src/main/resources/application-dev.yml", "a/b", Some("dev")),
            ("a/src/main/resources/application.yml", "a", None),
        ],
        "each source belongs to the NEAREST module root, not the outermost",
    );
    assert_eq!(
        corpus.profiles().into_iter().collect::<Vec<_>>(),
        vec!["dev"],
        "the census counts the profiles the corpus actually declares",
    );
    assert_eq!(corpus.module_of("a/b/anything.txt"), "a/b");
    assert_eq!(corpus.module_of("elsewhere/x.txt"), "", "an unclaimed path sits at the root");
}

#[test]
fn discover_flags_only_the_java_files_that_mention_the_properties_annotation() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let write = |rel: &str, body: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    };
    write("src/Bound.java", "@ConfigurationProperties(prefix = \"a\")\nclass Bound {}\n");
    write("src/Plain.java", "class Plain {}\n");

    let corpus = ConfigCorpus::discover(root);
    assert_eq!(
        corpus.props_candidates(),
        ["src/Bound.java"],
        "only the file carrying the needle is stashed for the class index",
    );
}

#[test]
fn a_profile_gated_document_is_not_yet_tagged_with_its_profile() {
    // A KNOWN GAP, pinned rather than hidden: Spring Boot >= 2.4 gates a
    // document with `spring.config.activate.on-profile`, and this flattener
    // reads the profile from the filename only. Both values therefore arrive
    // untagged. The day that changes, this test fails and says so — which is
    // the point of writing it down as an assertion instead of a comment.
    let facts = source_facts(
        "application.yml",
        "server:\n  url: https://dev.example.com\n\
         ---\n\
         spring:\n  config:\n    activate:\n      on-profile: prod\n\
         server:\n  url: https://prod.example.com\n",
    )
    .expect("a source");
    assert_eq!(facts.profile, None, "the filename is unprofiled, so the file is");
    let urls: Vec<&str> = facts
        .values
        .iter()
        .filter(|v| v.key == "server.url")
        .map(|v| v.value.as_str())
        .collect();
    assert_eq!(
        urls,
        vec!["https://dev.example.com", "https://prod.example.com"],
        "both documents' values survive — but neither carries `prod` (known gap)",
    );
}
