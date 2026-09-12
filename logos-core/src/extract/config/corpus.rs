//! The committed-configuration corpus: canonical key → value facts, tagged with
//! the profile that defines them ([FR-WS-19], [FR-CG-02], [ADR-64]).
//!
//! # A promotion, not a build ([CR-121] §5.1)
//!
//! Every symbol below was **moved verbatim** out of the S-365 measurement
//! harness (`logos-core/tests/operand_resolvability/configuration_agreement.rs`),
//! where it had been validated against an 84-member Spring estate — 174 sources,
//! 872 distinct keys, 5 profiles — and the 22 unit tests that covered exactly
//! these symbols moved with it into `mod tests` below, unmodified. (The rest of
//! that harness's roughly sixty tests cover the binding and resolution halves,
//! which have not been promoted; they stayed.) The YAML flattener in particular is a
//! **deliberate subset** whose every skip was learned from a real mis-read on
//! that estate (see [`parse_yaml`]); it is not to be rewritten, and a rewrite
//! that "simplifies" one of those skips reintroduces a fabricated key.
//!
//! # What is here, and what is not
//!
//! This module owns the **corpus**: discovering configuration sources, reading
//! their profile from the filename, and flattening each one to canonical
//! key → value pairs. It owns no resolution — binding a `@ConfigurationProperties`
//! accessor to a key, and judging what the corpus proves about that key, stay in
//! the measurement harness until their own stories promote them (S-381, S-382).
//!
//! # Two entry points, one flattener
//!
//! - [`ConfigCorpus::discover`] walks a corpus root and reads every source. It is
//!   the **measurement** entry point: the census figures downstream stories
//!   reconcile against are computed from it, and it is what `LOGOS_REF_WORKSPACE`
//!   tests drive.
//! - [`source_facts`] flattens **one already-read file**. It is the **ingestion**
//!   entry point, called from the config extraction pass with the source text
//!   that pass has already loaded, so ingestion adds no file IO of its own
//!   ([FR-WS-19] AC7). A file whose name is not a configuration source yields
//!   `None` and writes nothing, which is what keeps a member with no
//!   configuration corpus byte-for-byte unaffected.
//!
//! [ADR-64]: ../../../../docs/specs/architecture/decisions/ADR-64.md
//! [CR-121]: ../../../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
//! [FR-CG-02]: ../../../../docs/specs/requirements/FR-CG-02.md
//! [FR-WS-19]: ../../../../docs/specs/requirements/FR-WS-19.md

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The three configuration file stems Spring loads, and the profile-variant
/// forms of each. Names, not a glob: `bootstrap.yml`, `application.json` and a
/// `config/` override directory are out of scope and their absence is stated
/// rather than assumed (the corpus contains none).
const CONFIG_EXTENSIONS: [&str; 3] = ["yml", "yaml", "properties"];

/// Build descriptors whose directory is a module root — the scope one
/// deployable's configuration is assembled from.
const MODULE_DESCRIPTORS: [&str; 5] =
    ["pom.xml", "build.gradle", "build.gradle.kts", "package.json", "go.mod"];

/// One discovered configuration source.
#[derive(Debug, Clone)]
pub struct ConfigSource {
    /// Corpus-relative path.
    pub path: String,
    /// The `application-<profile>` profile, or `None` for the unprofiled file.
    pub profile: Option<String>,
    /// Module root this source belongs to (corpus-relative, `""` at the root).
    pub module: String,
    /// Canonical key → the values this file proves for it. A file that defines
    /// one key twice (multi-document YAML) proves both.
    pub values: BTreeMap<String, BTreeSet<String>>,
}

/// Every configuration source in the corpus, plus the module partition.
#[derive(Debug, Default)]
pub struct ConfigCorpus {
    pub sources: Vec<ConfigSource>,
    /// Module roots, longest-prefix-matched to place a file.
    modules: BTreeSet<String>,
    /// Java files mentioning `ConfigurationProperties`, stashed by the same
    /// walk so the class index needs no second traversal of the corpus.
    props_candidates: Vec<String>,
}


impl ConfigCorpus {
    /// Walk the corpus once: discover configuration sources, module roots, and
    /// the Java files that may declare a `@ConfigurationProperties` class.
    ///
    /// The walker is configured exactly as the parent module's `measure` walk —
    /// `parents(false)`, `git_global(false)`, `ignore(false)` — so the two
    /// passes admit the same population and a machine-local `~/.gitignore` can
    /// never change a published figure.
    pub fn discover(root: &Path) -> Self {
        let mut corpus = Self::default();
        let walker = ignore::WalkBuilder::new(root)
            .hidden(true)
            .git_ignore(true)
            .git_global(false)
            .ignore(false)
            .parents(false)
            .build();
        let mut raw: Vec<(String, Option<String>, String)> = Vec::new();
        for entry in walker.flatten() {
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(root) else {
                continue;
            };
            let rel = rel.to_string_lossy().replace('\\', "/");
            let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();

            if MODULE_DESCRIPTORS.contains(&name.as_str()) {
                corpus.modules.insert(parent_dir(&rel).to_string());
                continue;
            }
            if name.ends_with(".java") {
                if let Ok(text) = std::fs::read_to_string(entry.path()) {
                    if text.contains("ConfigurationProperties") {
                        corpus.props_candidates.push(rel.clone());
                    }
                }
                continue;
            }
            let Some(profile) = config_profile(&name) else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            raw.push((rel, profile, text));
        }
        // Module placement needs the full module set, so sources are built in a
        // second pass over what the walk collected — not a second walk.
        for (path, profile, text) in raw {
            let values = if path.ends_with(".properties") {
                parse_properties(&text)
            } else {
                parse_yaml(&text)
            };
            let module = corpus.module_of(&path).to_string();
            corpus.sources.push(ConfigSource { path, profile, module, values });
        }
        corpus.sources.sort_by(|a, b| a.path.cmp(&b.path));
        corpus
    }

    /// The module root owning a corpus-relative path: the longest module
    /// directory that is a path-segment prefix of it, or `""` at the root.
    pub fn module_of(&self, rel: &str) -> &str {
        self.modules
            .iter()
            .filter(|m| m.is_empty() || rel.starts_with(&format!("{m}/")))
            .max_by_key(|m| m.len())
            .map_or("", String::as_str)
    }

    /// Files the discovery walk flagged as possibly declaring a
    /// `@ConfigurationProperties` class, stashed by that same walk so the class
    /// index needs no second traversal.
    ///
    /// The needle is a Java literal today because that is what the harness this
    /// was promoted from measured; S-381 replaces it with a plugin-descriptor
    /// vocabulary, at which point this accessor's contract widens rather than
    /// changes. Read only by the measurement harness — production ingestion goes
    /// through [`source_facts`] and never runs this walk.
    pub fn props_candidates(&self) -> &[String] {
        &self.props_candidates
    }

    /// Profiles discovered across the corpus, for the census line.
    pub fn profiles(&self) -> BTreeSet<&str> {
        self.sources.iter().filter_map(|s| s.profile.as_deref()).collect()
    }
}

/// The directory part of a corpus-relative path (`""` for a root-level file).
fn parent_dir(rel: &str) -> &str {
    rel.rfind('/').map_or("", |i| &rel[..i])
}

/// The Spring profile of a configuration filename, or `None` when the file is
/// not a configuration source at all.
///
/// `Some(None)` is the unprofiled `application.<ext>`; `Some(Some(p))` is
/// `application-<p>.<ext>`. A hyphen inside the profile is kept whole, so
/// `application-it-jenkins.yml` is the profile `it-jenkins`, not `it`.
#[allow(clippy::option_option)]
pub fn config_profile(name: &str) -> Option<Option<String>> {
    let (stem, ext) = name.rsplit_once('.')?;
    if !CONFIG_EXTENSIONS.contains(&ext) {
        return None;
    }
    if stem == "application" {
        return Some(None);
    }
    let profile = stem.strip_prefix("application-")?;
    (!profile.is_empty()).then(|| Some(profile.to_string()))
}

/// Spring's relaxed binding, applied per `.`-segment: lower-case, and drop `-`
/// and `_`. `mailserver.api.uri-get-archive` and `mailserver.api.uriGetArchive`
/// canonicalise to the same key.
pub fn canonical_key(key: &str) -> String {
    key.split('.')
        .map(|segment| {
            segment
                .chars()
                .filter(|c| *c != '-' && *c != '_')
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// Strip a YAML inline comment, honouring quotes.
///
/// Separate from [`canonical_value`] and applied **before** the empty/block
/// tests in [`parse_yaml`], because a comment changes what those tests see:
/// `api:   # the mail API` is a mapping header, not a scalar, and reading it as
/// one re-parents its whole subtree.
///
/// A quoted scalar ends at its closing quote and anything after it is comment;
/// an unquoted one ends at the first ` #`. A bare `#` at the start is a
/// comment entire.
fn strip_yaml_comment(rest: &str) -> &str {
    let rest = rest.trim();
    for quote in ['"', '\''] {
        if let Some(body) = rest.strip_prefix(quote) {
            return match body.find(quote) {
                // Keep both quotes; `canonical_value` unwraps them.
                Some(end) => &rest[..end + 2],
                None => rest,
            };
        }
    }
    if rest.starts_with('#') {
        return "";
    }
    match rest.find(" #") {
        Some(hash) => rest[..hash].trim_end(),
        None => rest,
    }
}

/// A configuration value as the sources prove it: trimmed, with one layer of
/// matching quotes removed.
///
/// Comment stripping is **not** done here. It is a YAML rule and this runs for
/// `.properties` too, where `#` is only a comment at the start of a line and a
/// quote is a literal character — putting `.properties` values through the YAML
/// rules turned `app.url=/x # main` into `/x`, which is a different value from
/// the one Java's `Properties` reads and could make two sources falsely agree.
fn canonical_value(raw: &str) -> String {
    let text = raw.trim();
    let quoted = (text.starts_with('"') && text.ends_with('"') && text.len() >= 2)
        || (text.starts_with('\'') && text.ends_with('\'') && text.len() >= 2);
    if quoted {
        return text[1..text.len() - 1].to_string();
    }
    text.to_string()
}

// ── Configuration source parsing ────────────────────────────────────────────

/// Flatten the scalar subset of YAML the corpus's `application*.yml` files use:
/// nested mappings of scalars, `---` document separators, `#` comments.
///
/// A **deliberate subset**, not a YAML parser. Block scalars (`|`, `>`, and
/// their `|+`/`>-`/`|2` indicator forms) and sequences bind no scalar key here,
/// so a key whose value is a list is simply absent — which the agreement rule
/// then reports as `missing key` rather than as an agreed value it never read.
/// Under-reading is the safe direction: it can only lower the newly-admitted
/// count, never inflate it.
///
/// Both skips must consume the **body**, not just the header line. Skipping a
/// `-` item line while leaving the indent stack untouched let the item's own
/// continuation lines register as direct children of the enclosing mapping, so
/// a `routes:` list fabricated a real-looking `spring.cloud.gateway.routes.uri`
/// key from a list element — an invented value, not an absent one.
pub fn parse_yaml(text: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    // (indent, key) of every open mapping level.
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut skip_block_until: Option<usize> = None;
    let mut skip_sequence_until: Option<usize> = None;
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim();
        if let Some(block_indent) = skip_block_until {
            // A blank line inside a block scalar is part of it. Ending the skip
            // there would let the block's remaining lines register keys.
            if trimmed.is_empty() || indent > block_indent {
                continue;
            }
            skip_block_until = None;
        }
        if let Some(seq_indent) = skip_sequence_until {
            // The continuation lines of a `- name: x` item are indented past
            // the dash and are the ITEM's keys, not the parent mapping's.
            if trimmed.is_empty() || indent > seq_indent {
                continue;
            }
            skip_sequence_until = None;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed == "---" || trimmed.starts_with("--- ") {
            stack.clear();
            continue;
        }
        if trimmed.starts_with('-') {
            skip_sequence_until = Some(indent);
            continue;
        }
        let Some((key, rest)) = split_yaml_entry(trimmed) else {
            continue;
        };
        while stack.last().is_some_and(|(i, _)| *i >= indent) {
            stack.pop();
        }
        // Before anything is decided about `rest`: a trailing comment must not
        // make a mapping header look like a scalar.
        let rest = strip_yaml_comment(rest);
        if rest.is_empty() {
            stack.push((indent, key));
            continue;
        }
        // Any block-scalar indicator, not just the four bare spellings: `|+`,
        // `>-`, `|2` and friends all introduce a body that is not YAML. A plain
        // scalar never begins with `|` or `>`, and a quoted one took the branch
        // above.
        if rest.starts_with('|') || rest.starts_with('>') {
            skip_block_until = Some(indent);
            continue;
        }
        // An anchor or alias (`&name`, `*name`) is not a value: `defaults: &d`
        // is a mapping header whose children must stay under it.
        if rest.starts_with('&') {
            stack.push((indent, key));
            continue;
        }
        // A flow collection (`[a, b]`, `{k: v}`) is not a scalar. Recording its
        // source text as the value would let a list-valued key resolve to the
        // string "[a, b]" and be reported as agreed — a mis-read, not an
        // under-read, and the one direction this parser must not take.
        if rest.starts_with('[') || rest.starts_with('{') {
            continue;
        }
        let path: Vec<&str> =
            stack.iter().map(|(_, k)| k.as_str()).chain(std::iter::once(key.as_str())).collect();
        out.entry(canonical_key(&path.join(".")))
            .or_default()
            .insert(canonical_value(rest));
    }
    out
}

/// Split a YAML mapping line into its key and the rest, honouring a quoted key
/// and refusing a line with no `:` at all.
fn split_yaml_entry(line: &str) -> Option<(String, &str)> {
    for quote in ['"', '\''] {
        if let Some(rest) = line.strip_prefix(quote) {
            let end = rest.find(quote)?;
            let after = rest[end + 1..].trim_start();
            return Some((rest[..end].to_string(), after.strip_prefix(':')?));
        }
    }
    let colon = line.find(':')?;
    let key = line[..colon].trim();
    // A YAML key is one token. Anything with whitespace in it is the tail of a
    // construct this subset does not read — a flow mapping, or a continuation
    // line of a value that happens to contain a colon — and must not register
    // a key.
    if key.is_empty() || key.contains(char::is_whitespace) {
        return None;
    }
    Some((key.to_string(), &line[colon + 1..]))
}

/// Parse a `.properties` file: `key=value` or `key:value`, `#`/`!` comments,
/// and trailing-backslash line continuation.
///
/// Two rules that differ from YAML and cost a wrong value if borrowed from it:
/// a line continues only on an **odd** number of trailing backslashes (`a=C:\\`
/// is a value ending in one literal backslash, not a continuation that swallows
/// the next entry), and `#` is a comment only at the start of a line, never
/// inline. Values are therefore trimmed but not comment-stripped or unquoted.
pub fn parse_properties(text: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut pending = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if pending.is_empty() && (trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('!')) {
            continue;
        }
        let trailing_backslashes = trimmed.chars().rev().take_while(|c| *c == '\\').count();
        if trailing_backslashes % 2 == 1 {
            let head = &trimmed[..trimmed.len() - 1];
            pending.push_str(head.trim_end());
            continue;
        }
        let joined = format!("{pending}{trimmed}");
        pending.clear();
        let Some((key, value)) = joined
            .find(['=', ':'])
            .map(|i| (joined[..i].trim(), &joined[i + 1..]))
        else {
            continue;
        };
        if key.is_empty() {
            continue;
        }
        // Trim only: see the note above on why the YAML value rules must not
        // reach a `.properties` value.
        out.entry(canonical_key(key)).or_default().insert(value.trim().to_string());
    }
    out
}

// ── Ingestion entry point ───────────────────────────────────────────────────

/// One configuration source as the extract pass records it: the profile the
/// filename declares, and its flattened key → value pairs.
///
/// The ingestion-shaped mirror of [`ConfigSource`]. It carries no path (the
/// owning [`Facts`](crate::extract::Facts) already names the file) and no module
/// (a corpus-level partition a single-file pass cannot see), so it maps one-to-one
/// onto the `config_sources` / `config_values` rows migration 19 admits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSourceFact {
    /// The `application-<profile>` profile, or `None` for the unprofiled file.
    pub profile: Option<String>,
    /// Canonical key → value, sorted and deduplicated. A key a multi-document
    /// file defines twice contributes one pair per distinct value.
    pub values: Vec<ConfigValueFact>,
}

/// One canonical key → value pair proven by one configuration source.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConfigValueFact {
    /// The relaxed-binding canonical key (see [`canonical_key`]).
    pub key: String,
    /// The committed literal, trimmed and unquoted per the source's own rules.
    pub value: String,
}

/// Flatten one already-read file into its configuration facts, or `None` when
/// the file is not a configuration source.
///
/// `path` is project-relative; only its basename decides admission, by exactly
/// the [`config_profile`] rule [`ConfigCorpus::discover`] uses — so the ingested
/// population and the measured corpus are the same population, and the parser is
/// chosen by extension the same way. `text` is the source the caller already
/// holds: this function opens nothing.
pub fn source_facts(path: &str, text: &str) -> Option<ConfigSourceFact> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let profile = config_profile(name)?;
    let flat = if name.ends_with(".properties") {
        parse_properties(text)
    } else {
        parse_yaml(text)
    };
    let values = flat
        .into_iter()
        .flat_map(|(key, values)| {
            values
                .into_iter()
                .map(move |value| ConfigValueFact { key: key.clone(), value })
        })
        .collect();
    Some(ConfigSourceFact { profile, values })
}

#[cfg(test)]
mod tests {
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
    fn the_ingested_population_is_the_same_population_discover_admits() {
        // The two entry points must not drift: whatever `config_profile` admits
        // for the walk, `source_facts` admits for ingestion, and with the same
        // profile. A drift here would make the AC5 census and the indexed tables
        // disagree about the same estate.
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
}
