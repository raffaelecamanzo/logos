//! The committed-configuration corpus: canonical key → value facts, tagged with
//! the profile that defines them ([FR-WS-19], [FR-CG-02], [ADR-64]).
//!
//! # A promotion, not a build ([CR-121] §5.1)
//!
//! Every symbol below was **moved verbatim** out of the S-365 measurement
//! harness (`logos-core/tests/operand_resolvability/configuration_agreement.rs`),
//! where it had been validated against an 84-member Spring estate — 174 sources,
//! 872 distinct keys, 5 profiles — and the 22 unit tests that covered exactly
//! these symbols moved with it into `mod tests` below, unmodified. (The harness
//! had 74 tests; the other 52 cover the binding and resolution halves, which have
//! not been promoted, and stayed.) The YAML flattener in particular is a
//! **deliberate subset** whose every skip was learned from a real mis-read on
//! that estate (see [`parse_yaml`]); it is not to be rewritten, and a rewrite
//! that "simplifies" one of those skips reintroduces a fabricated key.
//!
//! # An accepted [NFR-MA-01] carve-out, recorded rather than implied
//!
//! Three constants below encode Spring/Java vocabulary in core rather than in a
//! plugin descriptor: [`CONFIG_EXTENSIONS`], [`MODULE_DESCRIPTORS`], and
//! [`config_profile`]'s `application-<profile>` stem. [NFR-MA-01] asks for no
//! per-language branching in `logos-core`, so this is a real exception and is
//! named as one. [CR-121] §5.1 sanctions the *content* ("a second language costs
//! an extractor rather than architecture"), and S-381 has since moved the
//! `@ConfigurationProperties` **annotation** vocabulary and the accessor
//! convention out to the plugin descriptor (see [`binding`](super::binding)) —
//! but it did not cover these three, and no other story does. They are carried
//! deliberately, with this note as their record, until a story claims them.
//!
//! # What is here, and what is not
//!
//! This module owns the **corpus**: discovering configuration sources, reading
//! their profile from the filename, and flattening each one to canonical
//! key → value pairs. It owns no resolution. Binding an accessor to a key is
//! [`binding`](super::binding), promoted by S-381; judging what the corpus
//! proves about that key stays in the measurement harness until S-382 promotes
//! it.
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
//! [NFR-MA-01]: ../../../../docs/specs/requirements/NFR-MA-01.md
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
    /// Every file the walk admitted, stashed so the binding index needs no
    /// second traversal of the corpus. Vocabulary-free on purpose — see
    /// [`ConfigCorpus::files`].
    files: Vec<String>,
}

impl ConfigCorpus {
    /// Walk the corpus once: discover configuration sources, module roots, and
    /// the file roster the binding index selects its candidates from.
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
            corpus.files.push(rel.clone());

            if MODULE_DESCRIPTORS.contains(&name.as_str()) {
                corpus.modules.insert(parent_dir(&rel).to_string());
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

    /// Every file the discovery walk admitted, in walk order, stashed by that
    /// same walk so a later pass needs no second traversal.
    ///
    /// **Vocabulary-free, and that is the point (S-381).** This roster used to
    /// be "the Java files whose text mentions `ConfigurationProperties`" — a
    /// Spring literal in `logos-core`, so a second language spelling its binding
    /// annotation differently could not be added without editing core, which is
    /// exactly what [FR-WS-19] and [NFR-MA-01] forbid. The selection now happens
    /// where the vocabulary lives: [`PropertiesIndex::build`] keeps the files
    /// whose extension its plugin claims and whose text mentions one of the
    /// plugin descriptor's own annotations
    /// ([`crate::plugin::PropertiesDescriptor::annotations`]).
    ///
    /// The file reads did not multiply, they moved: the walk no longer reads
    /// every `.java` file to test a needle, and the index reads exactly that
    /// same set once.
    ///
    /// Read only by the measurement harness and the binding index — production
    /// ingestion goes through [`source_facts`] and never runs this walk.
    ///
    /// [PropertiesIndex::build]: crate::extract::config::binding::PropertiesIndex::build
    /// [NFR-MA-01]: ../../../../docs/specs/requirements/NFR-MA-01.md
    pub fn files(&self) -> &[String] {
        &self.files
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

/// Whether `rest` opens a quoted scalar whose body carries an escape this module
/// does not decode: any `\` inside a double-quoted scalar, or a doubled `''`
/// inside a single-quoted one.
///
/// # Why refusing beats reading it
///
/// [`strip_yaml_comment`] ends a quoted scalar at the first inner quote, and
/// nothing here decodes escapes. On a body carrying one, that is not an
/// under-read but a **mis-read in the losing direction**: `"/api/\"alpha\"/v1"`
/// and `"/api/\"beta\"/v2"` both truncate to `/api/\`, so two sources committing
/// *different* values are recorded as committing the *same* one. That is a
/// fabricated agreement — the thing [FR-WS-19] means when it says disagreement
/// must be represented rather than averaged, and [NFR-RA-05] when it says never
/// fabricate.
///
/// # It costs no coverage on the reference estate
///
/// Measured: exactly **one** committed scalar is refused across all 174 sources
/// — `opentracing.spring.web.skip-pattern` in `mailbox-api`, whose regex carries
/// a `\` — and that canonical key is defined by **26** files, 25 of them without
/// an escape. So the key keeps its value from the other sources and the census
/// is unchanged at 872 distinct keys. Refusing here removes a wrong value, not a
/// key. (An earlier, cruder form of this guard tested the whole line for a
/// backslash, over-refused, and did cost a key — which is why the scan below
/// stops at the closing quote.)
///
/// So the key is emitted with **no value at all** rather than a wrong one, which
/// the agreement rule then reports as `missing key`. Under-reading is the safe
/// direction this module takes everywhere else (see [`parse_yaml`]); this makes
/// the quoted-scalar case take it too.
///
/// Decoding the escapes properly would keep the key *and* be correct, and is the
/// better answer whenever someone wants to write and test a YAML unescaper. This
/// is deliberately the smaller, provable change: it recognises, and declines.
///
/// Returns `false` for an unquoted scalar and for a quoted one needing no
/// decoding — the overwhelming majority, which keep their current behaviour
/// byte-for-byte. An unterminated quote also returns `false`, preserving what
/// [`strip_yaml_comment`] already does with it.
///
/// [FR-WS-19]: ../../../../docs/specs/requirements/FR-WS-19.md
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
fn quoted_scalar_carries_an_escape(rest: &str) -> bool {
    let rest = rest.trim();
    let mut chars = rest.chars();
    let Some(quote) = chars.next() else {
        return false;
    };
    match quote {
        // Double-quoted: `\` introduces an escape, so the first one decides.
        // Scanning stops at the closing quote so a `\` in a trailing comment —
        // outside the scalar — never counts.
        '"' => {
            for c in chars {
                match c {
                    '\\' => return true,
                    '"' => return false,
                    _ => {}
                }
            }
            false
        }
        // Single-quoted: the only escape YAML has here is a doubled `''`.
        '\'' => {
            let mut rest_chars = chars.peekable();
            while let Some(c) = rest_chars.next() {
                if c == '\'' {
                    return rest_chars.peek() == Some(&'\'');
                }
            }
            false
        }
        _ => false,
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
        // Before the comment strip, because the strip destroys the evidence: a
        // single-quoted `'it''s fine'` ends up as `'it'`, and by then nothing can
        // tell a doubled quote from a closing one. A quoted scalar carrying an
        // escape this module does not decode is refused outright rather than
        // recorded truncated — see [`quoted_scalar_carries_an_escape`] for why a
        // truncated value is worse than an absent one.
        if quoted_scalar_carries_an_escape(rest) {
            continue;
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
/// the [`config_profile`] rule [`ConfigCorpus::discover`] uses, and the parser is
/// chosen by extension the same way. `text` is the source the caller already
/// holds: this function opens nothing.
///
/// # The profile comes from the filename only
///
/// A Spring Boot >= 2.4 multi-document file can gate a document with
/// `spring.config.activate.on-profile`. This reads no such gate: every value in
/// the file carries the filename's profile, so a `prod`-gated override in an
/// unprofiled `application.yml` arrives as a second untagged value of the key,
/// indistinguishable from a genuine in-file duplicate. Reproduced end to end
/// during review; **0 of the 172** reference-estate files use `on-profile` or a
/// `---` separator, so it moves no published figure — but this flattener now runs
/// on every indexed project, not only that estate. `a_profile_gated_document_is_not_yet_tagged_with_its_profile`
/// in `mod tests` pins the current behaviour so the day it changes is visible.
///
/// # The ingested population is a SUBSET of the measured one
///
/// Applying the same rule does not make the two populations equal, because this
/// function is only ever *reached* for a file the plugin registry claims. No
/// descriptor claims `.properties` — a grammar for it does not exist — so
/// `is_config_admitted` never admits one, and the `parse_properties` arm below is
/// unreachable from the production pipeline. On the reference estate that is 31
/// of the 174 discovered sources. [`ConfigCorpus::discover`] walks the filesystem
/// itself and does read them, which is why the census and the tables count
/// different populations and why the census is measured through `discover`.
/// Closing the gap needs a `.properties` artifact plugin — registry work that no
/// story in this sprint owns.
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
#[path = "corpus_tests.rs"]
mod tests;
