//! The generic configuration-**binding** interpreter: a properties class, its
//! prefix, and the accessor that reads one of its properties
//! ([FR-WS-19], [FR-PL-02], [NFR-MA-01], [ADR-64]).
//!
//! # What "binding" means here
//!
//! [`corpus`](super::corpus) owns the committed *values*: which files define a
//! key and what each one proves about it. This module owns the other half — how
//! a use site NAMES one of those keys:
//!
//! ```text
//! mailServerConfigurationApi.getUriGetArchive()
//!   │                        └─ accessor → property `uriGetArchive`
//!   └─ field of declared type `MailServerConfigurationApi`
//!        └─ @ConfigurationProperties(prefix = "mailserver.api")
//!             → key `mailserver.api.uri-get-archive`
//! ```
//!
//! The two halves meet in [`corpus::canonical_key`](super::corpus::canonical_key):
//! the key this module spells and the key the corpus flattened are compared
//! under Spring's relaxed binding, so `uri-get-archive`, `uriGetArchive` and
//! `URI_GET_ARCHIVE` are one key.
//!
//! # Nothing here knows what Java looks like ([NFR-MA-01], [ADR-54])
//!
//! The interpreter this module ships reads **capture names and descriptor
//! rows**, never tree-sitter node kinds. Everything language-shaped lives in
//! two places a language owns outright:
//!
//! | what | where | Java's spelling |
//! |------|-------|-----------------|
//! | which tree shape is a bound class, its name, its prefix argument and its declared properties | the plugin's `properties` query | `plugins/java/queries/properties.scm` |
//! | which annotation marks one, and how a use site spells a read | the descriptor's `[properties]` table | `annotations` / `accessor_prefixes` |
//!
//! This is what it replaces. The S-365 measurement harness indexed properties
//! classes by walking tree-sitter Java nodes directly — `class_declaration`,
//! `modifiers`, `annotation`, `element_value_pair`, `field_declaration`,
//! `formal_parameter` — so a second language did not cost a query file, it cost
//! a second walk in core. That is the shape [NFR-MA-01] exists to forbid, and
//! [CR-121] §5.1 names its removal as what makes the substrate
//! language-agnostic rather than Java-shaped.
//!
//! The `invocations` capability (S-340) is the precedent followed here in its
//! entirety: one loop over the query's matches, one emission point after it, and
//! no judgment that a query could have made.
//!
//! # The capture contract
//!
//! A `properties.scm` binds these names; core reads nothing else. Every one is
//! grouped by the `@props.class` node, so a language is free to spell the class
//! header and its property list as separate patterns — which every grammar
//! needs, because one match cannot bind a class and all of its fields at once.
//!
//! | capture | what it binds | required |
//! |---------|---------------|----------|
//! | `@props.class` | the declaration node — the **grouping key**, never read for text | yes |
//! | `@props.class.name` | the node whose text is the class's simple name | yes |
//! | `@props.annotation` | the node whose text is an annotation's name; kept only when the descriptor's `annotations` lists it | yes |
//! | `@props.prefix` | the node holding the prefix — kept only when it is a static string literal **and its own match also binds an in-vocabulary `@props.annotation`** | no (its absence is counted, see [`PropertiesIndex::prefixless`]) |
//! | `@props.field` | the node whose text is one declared property's name | no (a class may declare none) |
//!
//! A match that binds no `@props.class` contributes nothing: without the
//! grouping key there is no declaration to attribute it to, and guessing one
//! from node position is the approximate match [NFR-RA-05] forbids.
//!
//! **A prefix belongs to the annotation captured beside it, not to the class.**
//! This is the one rule in the contract that is not per-declaration, and it is
//! load-bearing: a bound class routinely carries other argument-bearing
//! annotations (`@Validated`, `@RequestMapping(value = "/x")`, a
//! `@Component("bean")`), and a prefix read per *class* would take one of those
//! as the configuration prefix. Requiring the annotation in the same match makes
//! the query say which annotation it is reading, and the descriptor decide
//! whether that annotation binds — the same division as everywhere else here.
//!
//! # Two refusals, both resolving to nothing rather than to a guess
//!
//! - A **colliding simple name** — two declarations of `MailServerConfigurationApi`
//!   under different prefixes or property sets — is a [`PropertiesIndex::collisions`]
//!   entry, and [`PropertiesIndex::get`] returns `None` for it outside the module
//!   that declares it. This one is not theoretical: the reference estate declares
//!   that exact name more than once, and a workspace-wide "first wins" index
//!   resolved one member's getter against another member's class and reported
//!   eleven false refusals.
//! - An **ambiguous accessor** — one whose name yields two or more properties the
//!   class actually declares — is [`BindingRefusal::AmbiguousProperty`]. It exists
//!   because [`crate::plugin::PropertiesDescriptor::accessor_prefixes`] is a set rather than an
//!   ordered fallback: every entry is tried, and two survivors mean the source
//!   does not prove which property the site reads.
//!
//! [ADR-54]: ../../../../docs/specs/architecture/decisions/ADR-54.md
//! [ADR-64]: ../../../../docs/specs/architecture/decisions/ADR-64.md
//! [CR-121]: ../../../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
//! [FR-PL-02]: ../../../../docs/specs/requirements/FR-PL-02.md
//! [FR-WS-19]: ../../../../docs/specs/requirements/FR-WS-19.md
//! [NFR-MA-01]: ../../../../docs/specs/requirements/NFR-MA-01.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use tree_sitter::{Parser, QueryCursor, StreamingIterator};

use crate::plugin::{LanguagePlugin, LanguageRegistry};

use super::corpus::{canonical_key, ConfigCorpus};

/// The capability whose query this module interprets.
pub const PROPERTIES_CAPABILITY: &str = "properties";

/// One configuration-bound class: its prefix and the property names it declares.
///
/// Language-neutral by construction — every field below is filled from a
/// capture name, so a class declared in Kotlin and one declared in Java are the
/// same value with the same meaning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertiesClass {
    /// The class's **simple** name, as the use site spells its declared type.
    pub name: String,
    /// The configuration key prefix the binding annotation declares.
    pub prefix: String,
    /// Canonicalised property names ([`canonical_key`]), so an accessor matches
    /// without re-deriving the source spelling.
    pub properties: BTreeSet<String>,
    /// The corpus-relative file declaring it.
    pub file: String,
    /// The module root declaring it — the scope a use site prefers.
    pub module: String,
    /// The plugin (language) that captured it, so
    /// [`bind`](PropertiesIndex::bind) reads this declaration under its OWN
    /// language's accessor convention rather than under a borrowed one.
    pub language: String,
}

/// What an accessor resolved to: the key, and the evidence for it.
///
/// [FR-WS-19]'s statement spells the resolution as a chain — *accessor → field →
/// owning class → annotation prefix → canonical key* — and every link of it is
/// recorded here rather than collapsed into the key it produced, so a consumer
/// can say **why** a site names the key it names.
///
/// [FR-WS-19]: ../../../../docs/specs/requirements/FR-WS-19.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyBinding {
    /// The configuration key, spelled `<prefix>.<property>` in the source's own
    /// spelling so a report names something a reader can find in the yaml. Match
    /// it against the corpus with [`canonical_key`].
    pub key: String,
    /// The canonicalised property the accessor named — the **field**.
    pub property: String,
    /// The **owning class**'s simple name.
    pub class: String,
    /// The file declaring the owning class.
    pub file: String,
}

/// Why an accessor bound no key. Each variant is a distinct, countable fault
/// rather than a bucket labelled "other" ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BindingRefusal {
    /// No accessor convention applies to the name at all, so it names no
    /// property — a Java `compute()` under `["get", "is"]`.
    NotAnAccessor,
    /// A convention applies, but the class declares no property of that name.
    PropertyNotDeclared,
    /// Two or more conventions each yield a property the class **does** declare.
    /// The source does not prove which one the site reads, so neither is used.
    AmbiguousProperty,
}

/// Every configuration-bound class in the corpus, by **simple** type name — each
/// name keeping *every* declaration of it.
///
/// Simple names, not fully-qualified ones: a field's declared type is written
/// unqualified at the use site, and resolving imports would be a second
/// resolver. The corpus makes that cheap reading unsafe on its own, so the
/// lookup is **module-scoped first** — see [`PropertiesIndex::get`].
#[derive(Debug, Default)]
pub struct PropertiesIndex {
    classes: BTreeMap<String, Vec<PropertiesClass>>,
    /// Language name → that language's accessor convention, for every plugin
    /// this index has been declared over ([`for_plugins`](Self::for_plugins)) or
    /// has absorbed a source through. A corpus that declares no bound class at
    /// all still knows what an accessor looks like, so a use site there refuses
    /// as "no class for that type" rather than as "not an accessor", which is a
    /// different fault and counted separately.
    ///
    /// **Keyed by language, never unioned, and that distinction is load-bearing.**
    /// An earlier form held one flat union of every declared convention, on the
    /// reasoning that a union can only *widen* the candidate set and a widened
    /// set with two survivors refuses rather than guesses. That reasoning holds
    /// for [`bind`](Self::bind) and fails completely for
    /// [`names_an_accessor`](Self::names_an_accessor): the empty prefix means
    /// *every* name is already a property name, so one language declaring it
    /// made the shape predicate vacuously true for **all** languages, and
    /// [`BindingRefusal::NotAnAccessor`] became unreachable in any index built
    /// over a registry carrying such a language — which is every default build,
    /// whether or not the corpus holds one file of it. A whole refusal category
    /// silently emptied into its neighbours. Judging each declaration under its
    /// own language's convention is both correct and narrower.
    conventions: BTreeMap<String, BTreeSet<String>>,
    /// Simple names whose declarations disagree; each resolves to nothing.
    pub collisions: BTreeSet<String>,
    /// Classes carrying a binding annotation this index could not key, counted
    /// rather than dropped. Three causes reach it, and they are all *readings of
    /// the prefix*, never an absent class:
    ///
    /// 1. a **marker** annotation — `@ConfigurationProperties` with no arguments;
    /// 2. a prefix that is **not a static literal** — a constant reference or a
    ///    concatenation, which the shared literal reader declines;
    /// 3. **two distinct readable prefixes** on one declaration, which prove
    ///    neither ([NFR-RA-05]). This one is a *refusal* folded into an absence
    ///    counter; it is reachable (`@ConfigurationProperties(prefix = "a",
    ///    value = "b")` is legal Java that Spring rejects only at runtime) and
    ///    it is zero on the reference estate.
    ///
    /// The annotation on a `@Bean` **factory method** is deliberately NOT here:
    /// no pattern matches such a declaration at all, so it is invisible rather
    /// than counted — see the stated ceilings in each `properties.scm`. (The
    /// walk this replaced carried the same doc, and it was wrong there too.)
    ///
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub prefixless: usize,
}

impl PropertiesIndex {
    /// An empty index that already knows the accessor conventions of `plugins`.
    ///
    /// The constructor to reach for when sources are fed in one at a time: it
    /// separates *which conventions this index judges by* from *which classes it
    /// happens to hold*, so the two cannot be conflated. [`Self::default`] knows
    /// no convention and therefore binds nothing — correct, but only ever what
    /// an index over zero languages should do.
    pub fn for_plugins(plugins: &[&dyn LanguagePlugin]) -> Self {
        let mut index = Self::default();
        for plugin in plugins {
            index.declare(*plugin);
        }
        index
    }

    /// Adopt one plugin's accessor convention, under its own language name,
    /// without absorbing any source.
    fn declare(&mut self, plugin: &dyn LanguagePlugin) {
        if let Some(descriptor) = plugin.semantics().properties.as_ref() {
            self.conventions
                .entry(plugin.name().to_string())
                .or_default()
                .extend(descriptor.accessor_prefixes.iter().cloned());
        }
    }

    /// Index every bound class the registry's binding languages declare across
    /// the corpus.
    ///
    /// Which languages bind is the **registry's** answer, not a roster held
    /// here: every loaded plugin declaring the [`PROPERTIES_CAPABILITY`]
    /// participates, so a language joins by shipping a descriptor and a query.
    /// Which plugin owns a file is [`LanguageRegistry::for_path`]'s answer, the
    /// same admission rule the extract pass uses — a second, hand-rolled
    /// extension test here would be a matcher free to disagree with it about
    /// case, about a leading dot, and about the basename claims.
    ///
    /// The candidate selection is then the descriptor's: a file is parsed only
    /// when its text mentions one of its own plugin's
    /// [`annotations`](crate::plugin::PropertiesDescriptor::annotations). That
    /// substring test is a **pre-filter and nothing more** — the query and the
    /// exact-match vocabulary test decide what actually indexes — so it can only
    /// save a parse, never admit one.
    pub fn build(root: &Path, corpus: &ConfigCorpus, registry: &LanguageRegistry) -> Self {
        let binders: Vec<&dyn LanguagePlugin> = registry
            .iter()
            .filter(|p| p.capabilities().iter().any(|c| c == PROPERTIES_CAPABILITY))
            .collect();
        let mut index = Self::for_plugins(&binders);
        for rel in corpus.files() {
            let Some(plugin) = registry.for_path(rel) else {
                continue;
            };
            let Some(descriptor) = plugin.semantics().properties.as_ref() else {
                continue;
            };
            if !plugin.capabilities().iter().any(|c| c == PROPERTIES_CAPABILITY) {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(root.join(rel)) else {
                continue;
            };
            if !descriptor.annotations.iter().any(|a| source.contains(a)) {
                continue;
            }
            let module = corpus.module_of(rel).to_string();
            index.absorb_source(plugin, rel, &module, &source);
        }
        index.seal();
        index
    }

    /// Index one already-read source through its plugin's `properties` query.
    ///
    /// The ingestion-shaped entry point, and the one the unit tests drive: it
    /// adds no file IO of its own, exactly as
    /// [`source_facts`](super::corpus::source_facts) does for the corpus half.
    /// A plugin shipping no `properties` capability, no `[properties]` table, or
    /// a source the query does not match, adds nothing and is not an error
    /// (absence is not a fault — [NFR-MA-01]).
    ///
    /// Call [`seal`](Self::seal) once after the last source.
    ///
    /// [NFR-MA-01]: ../../../../docs/specs/requirements/NFR-MA-01.md
    pub fn absorb_source(
        &mut self,
        plugin: &dyn LanguagePlugin,
        rel: &str,
        module: &str,
        source: &str,
    ) {
        let Some(descriptor) = plugin.semantics().properties.as_ref() else {
            return;
        };
        let Some(query) = plugin.query(PROPERTIES_CAPABILITY) else {
            return;
        };
        let mut parser = Parser::new();
        if parser.set_language(plugin.language()).is_err() {
            return;
        }
        let Some(tree) = parser.parse(source, None) else {
            return;
        };
        // Idempotent, and here as well as in `for_plugins` so a caller that
        // feeds sources without declaring first still judges by the right
        // convention.
        self.declare(plugin);

        let src = source.as_bytes();
        let capture_names = query.capture_names();
        // One loop over the matches, accumulating into per-declaration drafts
        // keyed by the `@props.class` node, and ONE emission point after it
        // (S-340's shape). Grouping is what lets a language spell the class
        // header and its property list as separate patterns — no grammar binds a
        // declaration and all of its fields in a single match.
        let mut order: Vec<usize> = Vec::new();
        let mut drafts: HashMap<usize, Draft> = HashMap::new();
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(query, tree.root_node(), src);
        while let Some(m) = matches.next() {
            let Some(anchor) = m
                .captures
                .iter()
                .find(|c| capture_names[c.index as usize] == "props.class")
            else {
                continue;
            };
            let id = anchor.node.id();
            // Whether THIS match reads a binding annotation decides what its
            // prefix capture means — see the capture contract's prefix rule.
            let binding_match = m.captures.iter().any(|c| {
                capture_names[c.index as usize] == "props.annotation"
                    && c.node.utf8_text(src).is_ok_and(|text| {
                        descriptor.annotations.iter().any(|v| v == text.trim())
                    })
            });
            let draft = match drafts.entry(id) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(e) => {
                    order.push(id);
                    e.insert(Draft::default())
                }
            };
            for capture in m.captures {
                match capture_names[capture.index as usize] {
                    "props.class.name" => {
                        if let Ok(text) = capture.node.utf8_text(src) {
                            draft.name.get_or_insert_with(|| text.trim().to_string());
                        }
                    }
                    "props.annotation" => {
                        if let Ok(text) = capture.node.utf8_text(src) {
                            draft.annotations.insert(text.trim().to_string());
                        }
                    }
                    // The prefix of an annotation that does NOT bind is another
                    // annotation's argument, not this class's key prefix.
                    "props.prefix" if binding_match => {
                        // The shared literal reader every capture arm uses, so a
                        // grammar's string-content spelling is recognised in one
                        // place. A non-literal prefix (a constant, a
                        // concatenation) reads as absent, which is what makes the
                        // class `prefixless` rather than wrongly keyed.
                        if let Some(prefix) =
                            crate::extract::static_string_literal(capture.node, src)
                        {
                            draft.prefixes.insert(prefix);
                        }
                    }
                    "props.field" => {
                        if let Ok(text) = capture.node.utf8_text(src) {
                            draft.properties.insert(canonical_key(text.trim()));
                        }
                    }
                    _ => {}
                }
            }
        }

        for id in order {
            let Some(draft) = drafts.remove(&id) else {
                continue;
            };
            // The vocabulary test, EXACT and in one place: the query captures
            // every annotation on the declaration, the descriptor decides which
            // ones bind. A query carrying its own `#eq?` would be a second copy
            // of this list, free to drift from it.
            if !draft
                .annotations
                .iter()
                .any(|a| descriptor.annotations.iter().any(|v| v == a))
            {
                continue;
            }
            let Some(name) = draft.name.filter(|n| !n.is_empty()) else {
                continue;
            };
            // Two readable prefixes on one declaration prove neither: the source
            // does not say which one keys, so the class is counted with the
            // prefixless ones rather than keyed on a guess (NFR-RA-05).
            let mut prefixes = draft.prefixes.into_iter();
            let (Some(prefix), None) = (prefixes.next(), prefixes.next()) else {
                self.prefixless += 1;
                continue;
            };
            self.classes.entry(name.clone()).or_default().push(PropertiesClass {
                name,
                prefix,
                properties: draft.properties,
                file: rel.to_string(),
                module: module.to_string(),
                language: plugin.name().to_string(),
            });
        }
    }

    /// Seal the index: a simple name whose declarations disagree — outside the
    /// module that will ask for it — is recorded as a collision.
    pub fn seal(&mut self) {
        for (name, declarations) in &self.classes {
            let distinct: BTreeSet<(&String, &BTreeSet<String>)> =
                declarations.iter().map(|c| (&c.prefix, &c.properties)).collect();
            if distinct.len() > 1 {
                self.collisions.insert(name.clone());
            }
        }
    }

    /// The class a use site in `module` sees: its own module's declaration
    /// first, then the workspace's when every remaining declaration agrees.
    pub fn get(&self, simple_type: &str, module: &str) -> Option<&PropertiesClass> {
        let declarations = self.classes.get(simple_type)?;
        // The own-module subset gets the same distinct-declaration test as the
        // workspace one: two classes of the same simple name in different
        // packages of ONE module is the same ambiguity the collision rule
        // exists for, and picking the first is the guess it forbids.
        let mut own = declarations.iter().filter(|c| c.module == module);
        if let Some(first) = own.next() {
            let ambiguous = own.any(|c| c.prefix != first.prefix || c.properties != first.properties);
            return (!ambiguous).then_some(first);
        }
        (!self.collisions.contains(simple_type)).then(|| declarations.first()).flatten()
    }

    /// Whether `language`'s convention reads `accessor` as naming a property.
    ///
    /// The **shape** question, asked without a class — a use site whose member
    /// read is not an accessor at all is a different fault from one whose class
    /// declares no such property, and the two are counted separately. It says
    /// nothing about whether a class declares what the name yields; that is
    /// [`bind`](Self::bind)'s answer.
    ///
    /// `language` is a plugin name ([`LanguagePlugin::name`]). A language this
    /// index was never declared over recognises nothing, which is the honest
    /// answer rather than a borrowed one.
    pub fn names_an_accessor(&self, language: &str, accessor: &str) -> bool {
        !self.candidates(language, accessor).is_empty()
    }

    /// Resolve `accessor` against `class`: **accessor → field → owning class →
    /// prefix → canonical key**, by name transformation alone — the chain
    /// [FR-WS-19]'s statement spells out.
    ///
    /// Every convention of the class's **own language** is tried and the results
    /// are intersected with what the class declares. Exactly one survivor binds;
    /// none is a refusal naming which half failed; two or more resolve to
    /// **nothing**, because a table order is not evidence ([FR-WS-19] AC3).
    ///
    /// [FR-WS-19]: ../../../../docs/specs/requirements/FR-WS-19.md
    pub fn bind(
        &self,
        class: &PropertiesClass,
        accessor: &str,
    ) -> Result<PropertyBinding, BindingRefusal> {
        let candidates = self.candidates(&class.language, accessor);
        if candidates.is_empty() {
            return Err(BindingRefusal::NotAnAccessor);
        }
        let mut declared = candidates
            .into_iter()
            .filter(|(property, _)| class.properties.contains(property));
        match (declared.next(), declared.next()) {
            (Some((property, spelled)), None) => Ok(PropertyBinding {
                key: format!("{}.{spelled}", class.prefix),
                property,
                class: class.name.clone(),
                file: class.file.clone(),
            }),
            (Some(_), Some(_)) => Err(BindingRefusal::AmbiguousProperty),
            _ => Err(BindingRefusal::PropertyNotDeclared),
        }
    }

    /// Every `(canonical property, source spelling)` an accessor name yields
    /// under `language`'s convention, before the class is consulted.
    ///
    /// The result is keyed by the CANONICAL property, which is where the dedup
    /// happens: two prefixes that agree on a property (a descriptor listing both
    /// `get` and `get_`) are one candidate, not a fabricated ambiguity. Last
    /// write wins over a `BTreeSet` walk, so the surviving spelling is
    /// deterministic.
    fn candidates(&self, language: &str, accessor: &str) -> BTreeMap<String, String> {
        let accessor = accessor.trim();
        let Some(prefixes) = self.conventions.get(language) else {
            return BTreeMap::new();
        };
        prefixes
            .iter()
            .filter_map(|prefix| {
                // The empty prefix is direct property access: the name already IS
                // the property, so nothing is stripped and nothing is re-cased.
                if prefix.is_empty() {
                    return (!accessor.is_empty()).then(|| accessor.to_string());
                }
                let stripped = accessor.strip_prefix(prefix.as_str())?;
                // The convention's suffix with its leading capital lowered —
                // `getUriGetArchive` names `uriGetArchive`, which relaxed binding
                // then matches against `uri-get-archive`. Only the SPELLING
                // changes; the canonical key is the same either way.
                let mut chars = stripped.chars();
                let first = chars.next()?;
                Some(first.to_lowercase().collect::<String>() + chars.as_str())
            })
            .map(|spelled| (canonical_key(&spelled), spelled))
            .collect()
    }

    /// Distinct class names indexed.
    pub fn len(&self) -> usize {
        self.classes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }
}

/// One declaration under construction, accumulated across the matches that bind
/// its `@props.class` node.
#[derive(Debug, Default)]
struct Draft {
    name: Option<String>,
    annotations: BTreeSet<String>,
    prefixes: BTreeSet<String>,
    properties: BTreeSet<String>,
}

#[cfg(all(test, feature = "lang-java"))]
#[path = "binding_tests.rs"]
mod tests;
