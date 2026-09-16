//! The **use-site** half of configuration binding: reading a captured operand
//! as an accessor on a configuration-bound class, and naming the canonical key
//! it resolves to ([FR-WS-19], [NFR-MA-01], [NFR-RA-05], [ADR-64]).
//!
//! [`binding`](super::binding) owns the *declaration* half — which classes are
//! configuration-bound, what prefix each carries, and which property an accessor
//! name yields. This module owns the one question that is left, and it is the
//! question the invocation arm actually asks:
//!
//! ```text
//! restClient.get().uri(mailServerConfigurationApi.getUriGetArchive())
//!                      │                          └─ the accessor NAME
//!                      └─ the RECEIVER, whose declared type this file states
//! ```
//!
//! The receiver may also be **qualified by the enclosing instance**, which is
//! how the reference estate writes essentially all of them (S-398):
//!
//! ```text
//! restClient.get().uri(this.mailboxApiProperties.getUriGetMailbox())
//!                      └┬─┘ └────────┬─────────┘
//!                       │            └─ the FIELD, whose declared type this
//!                       │               file states — the same fact as above
//!                       └─ the language's self reference, read from the
//!                          `[properties]` descriptor's `self_references`
//!                          row, never named here
//! ```
//!
//! That second shape needed no new hop: the field's declared type was already in
//! [`DeclaredTypes`] (a `private Foo bar;` declarator is what its one-hop rule
//! is written for), and only the receiver's *text* refused the site.
//!
//! Answering it needs one fact the declaration half does not hold: the **simple
//! type** the reading file declares for the receiver name. That is what
//! [`DeclaredTypes`] reads, and it is the whole of this module's tree work.
//!
//! # Grammar FIELD names only, never node kinds ([NFR-MA-01])
//!
//! [`binding`](super::binding) is driven by capture names and descriptor rows;
//! it names no grammar node kind at all, and a structural guard proves it. This
//! module cannot be driven by a query — the operand it reads is the node the
//! *invocations* query already captured, and re-capturing it through a second
//! query would make two languages' worth of `.scm` disagree about which node the
//! arm judged. So it reads the parse tree directly, and stays language-neutral a
//! different way: **only tree-sitter field names**, which are the structural
//! roles a grammar author declares, never the node kinds a grammar happens to
//! spell.
//!
//! | field | what it answers |
//! |-------|-----------------|
//! | `name` | the name a node declares, and the accessor a call names |
//! | `type` | the declared type of the node that carries it, or of its parent |
//! | `object` | the receiver a member call reads through |
//! | `parameters` / `body` / `arguments` | a node that **calls, or declares a callable** — never a value binding, so its `name` is skipped |
//!
//! The [NFR-MA-01] structural guard beside the declaration half
//! (`binding_tests::the_interpreter_names_no_jvm_grammar_node_kind`) covers this
//! file too, and its allowlist is empty for both: none of the six field names
//! above is a node kind in either loaded JVM grammar. `type` is the near miss —
//! `tree-sitter-kotlin-ng` declares it in `node-types.json`, but as a supertype
//! rather than a parser node kind — and the guard's own comment records what to
//! do if a grammar bump ever changes that.
//!
//! # Only a grammar that field-names its member calls is reached at all
//!
//! [`member_call`] needs an `object` field and a `name` field on the operand
//! node. Java's `method_invocation` has both. **Kotlin's grammar has neither** —
//! `plugins/kotlin/queries/invocations.scm` records that a receiver call is
//! `(call_expression (navigation_expression …) (value_arguments))` with *no
//! fields at all*, so every constraint it writes is positional. Kotlin is one of
//! the two languages shipping the `properties` capability, and a Kotlin **use
//! site** therefore resolves to nothing today; a Kotlin-declared **class** read
//! from a Java file resolves normally, which is the case
//! `binding_tests`/`accessor_tests` exercise.
//!
//! Recorded as the largest ceiling here rather than left to be discovered.
//! Closing it means giving the `[properties]` descriptor the field names — or a
//! positional reading — as data, which is the same change that would let a
//! grammar spelling its receiver `receiver:` (Ruby) or `operand:` (Go)
//! participate.
//!
//! # One shape still binds that should not, and no field separates it
//!
//! A Java `annotation_type_element_declaration` — the `Props api();` inside an
//! `@interface` — field-names exactly `name`, `type`, `dimensions` and an
//! optional `value`. A `formal_parameter` field-names `name`, `type` and
//! `dimensions`. **With no default clause the two are field-identical**, so a
//! rule driven by field names cannot tell an annotation element (a callable)
//! from a parameter (a value binding), and the element registers `api → Props`.
//!
//! It is recorded rather than worked around ([ADR-54]): separating them needs a
//! node-kind or parent-kind test, which is the language-shaped reading
//! [NFR-MA-01] forbids here. Its reach is narrow and bounded on both sides — the
//! fabricated name is only ever consulted if a use site *in the same file*
//! reads through a receiver of that name that the file otherwise never declares,
//! and any real declaration of it under a different type poisons the entry to
//! nothing rather than losing to it.
//!
//! # Everything unproven resolves to nothing ([NFR-RA-05])
//!
//! Each hop below returns [`None`] rather than a guess, and the arm then leaves
//! the site exactly where it already was — marked as a runtime-composed path.
//! There is no new refusal reason and no new counter: a site this module cannot
//! resolve is indistinguishable from one it was never asked about, which is what
//! makes wiring it in a widening of what is *captured* and never of what is
//! *believed*.
//!
//! [ADR-54]: ../../../../docs/specs/architecture/decisions/ADR-54.md
//! [ADR-64]: ../../../../docs/specs/architecture/decisions/ADR-64.md
//! [FR-WS-19]: ../../../../docs/specs/requirements/FR-WS-19.md
//! [NFR-MA-01]: ../../../../docs/specs/requirements/NFR-MA-01.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::BTreeMap;

use tree_sitter::Node;

use super::binding::PropertiesIndex;
use super::corpus::canonical_key;

/// The grammar field naming what a node declares, and what a member call reads.
const NAME_FIELD: &str = "name";
/// The grammar field naming a declared type.
const TYPE_FIELD: &str = "type";
/// The grammar field naming a member call's receiver.
const OBJECT_FIELD: &str = "object";
/// The grammar field naming a call's argument list — read by
/// [`BindingView::placeholder_for`] to refuse a getter-named call that is
/// invoked with arguments, and so cannot be the property getter it resembles.
const ARGUMENTS_FIELD: &str = "arguments";
/// The fields that mark a node as **calling or declaring a callable** rather
/// than declaring a value. Such a node carries a `name` and can reach a `type`
/// — its own return type, or its parent's — and binding the two would register
/// a method's name as a value of that type: a binding no source makes.
///
/// `arguments` is here because of a reproduced fabrication, not for symmetry. A
/// Java `method_invocation` field-names `name`, `object` and `arguments` and
/// carries no `parameters` or `body`; as the `value` of a `cast_expression`,
/// whose `type` field the parent hop reads, `(Props) reg.lookup()` registered
/// `lookup → Props`. End to end that turned a correct refusal into a confident
/// `config-bound` target on a receiver the file never declares — the arm
/// *believing* more rather than capturing more, which is the one thing
/// [NFR-RA-05] forbids. Deleting an unrelated cast elsewhere in the file flipped
/// the answer back.
///
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
const CALLABLE_FIELDS: [&str; 3] = [PARAMETERS_FIELD, "body", "arguments"];

/// The grammar field naming a callable's parameter list — the one member of
/// [`CALLABLE_FIELDS`] that is also a **scope** marker, and so is named
/// separately for [`field_position`] to read. Every node that introduces a
/// callable scope carries it; a type's own member declarations do not.
const PARAMETERS_FIELD: &str = "parameters";

/// Every name this file binds to a declared **simple** type name.
///
/// File-scoped and scope-blind, exactly like the corpus half's module scope: it
/// answers "what type does this file say this name has", not "which binding is
/// live here". That is why a name the file declares under **two different**
/// types resolves to nothing — see [`DeclaredTypes::get`].
#[derive(Debug, Default)]
pub struct DeclaredTypes {
    /// name → the single simple type it is declared with, or [`None`] when the
    /// file declares it with two that disagree.
    by_name: BTreeMap<String, Option<String>>,
    /// The same, restricted to names declared at a **field position** — outside
    /// every callable the file declares (S-398).
    ///
    /// A separate map rather than a flag on the one above, because the two
    /// answer different questions and poison **independently**: a field `api`
    /// beside a disagreeing local `api` leaves [`get`](Self::get) with nothing
    /// (correctly — a bare `api` is a scope question this walk cannot answer)
    /// while [`field`](Self::field) still answers, because `this.api` is not a
    /// scope question at all.
    fields_by_name: BTreeMap<String, Option<String>>,
}

impl DeclaredTypes {
    /// Read every `name` → declared-type binding in the file.
    ///
    /// The rule, in full: a node that carries a `name` field whose child is a
    /// leaf takes its type from its **own** `type` field, or failing that from
    /// its parent's — one hop, which is what a C-family declarator needs
    /// (`private Foo bar;` field-names the type on the declaration and the name
    /// on the declarator) and what a parameter does not (it carries both). A
    /// node that declares a callable is skipped entirely.
    ///
    /// Every binding is recorded twice when it is declared at a **field
    /// position** — see [`field_position`] for what that means and what it
    /// costs — so that [`field`](Self::field) can answer the one question
    /// [`get`](Self::get) must not: *what does `this.<name>` denote?*
    pub fn build(root: Node<'_>, src: &[u8]) -> Self {
        let mut types = Self::default();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));

            if CALLABLE_FIELDS
                .iter()
                .any(|f| node.child_by_field_name(f).is_some())
            {
                continue;
            }
            let Some(name_node) = node.child_by_field_name(NAME_FIELD) else {
                continue;
            };
            // A leaf, so a qualified or generic name node is not read as though
            // it were the identifier a use site spells.
            if name_node.named_child_count() > 0 {
                continue;
            }
            let Some(name) = name_node.utf8_text(src).ok().map(str::trim) else {
                continue;
            };
            let declared = node
                .child_by_field_name(TYPE_FIELD)
                .or_else(|| node.parent()?.child_by_field_name(TYPE_FIELD))
                .and_then(|n| n.utf8_text(src).ok())
                .map(simple_type_name)
                .filter(|t| !t.is_empty());
            let Some(declared) = declared else {
                continue;
            };
            record(&mut types.by_name, name, declared);
            if field_position(node) {
                record(&mut types.fields_by_name, name, declared);
            }
        }
        types
    }

    /// The simple type this file declares for `name`, when it declares exactly
    /// one — **at any position**, which is why a use site that names a scope
    /// must not ask this one.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.by_name.get(name)?.as_deref()
    }

    /// The simple type this file declares for `name` **as a field**, when it
    /// declares exactly one (S-398).
    ///
    /// The lookup a self-qualified receiver makes, and the reason it is separate
    /// from [`get`](Self::get) is a reproduced over-capture, not symmetry.
    /// `this.<name>` denotes a field of the enclosing class; [`get`](Self::get)
    /// is scope-blind and answers from parameters and locals too. Where the
    /// field is **inherited** — declared in another file, so nothing in this one
    /// poisons the entry — a same-named local of a *different* bound type
    /// answered for it, and the site bound a key the source never named. Three
    /// independent reviews reproduced it end to end.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields_by_name.get(name)?.as_deref()
    }
}

/// Record one `name` → `declared` binding, poisoning a disagreement.
///
/// A second, DISAGREEING declaration poisons the entry rather than losing to (or
/// beating) the first: which one a use site sees is a scope question this walk
/// cannot answer, and picking either would be the guess [NFR-RA-05] forbids.
///
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
fn record(into: &mut BTreeMap<String, Option<String>>, name: &str, declared: &str) {
    into.entry(name.to_string())
        .and_modify(|held| {
            if held.as_deref() != Some(declared) {
                *held = None;
            }
        })
        .or_insert_with(|| Some(declared.to_string()));
}

/// Whether `node` declares at a **field position** — that is, whether it sits
/// outside every callable its file declares.
///
/// Expressed the only way this module is allowed to express it: by a grammar
/// **field name**, never a node kind ([NFR-MA-01]). A node that declares or
/// calls a callable field-names `parameters`, so a declaration with no such
/// ancestor is not inside a method, a constructor or a lambda — it is a member
/// of the type. That is exactly the separation the C-family shapes need, because
/// the one-hop type rule cannot make it: a field (`private Foo bar;`) and a
/// local (`Foo bar = …;`) are *field-identical*, both taking their type from the
/// parent declaration, and only their enclosing scope tells them apart.
///
/// **What it costs, stated rather than left to be discovered.** A language whose
/// member declarations sit under a `parameters`-bearing node reads as having no
/// fields at all — a Java `record`'s components are the shipped example, since
/// `record_declaration` field-names `parameters`. Such a declaration resolves to
/// nothing through the self-qualified arm. That is the refusing direction, so it
/// is a miss and never a fabrication ([NFR-RA-05]).
///
/// [NFR-MA-01]: ../../../../docs/specs/requirements/NFR-MA-01.md
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
fn field_position(node: Node<'_>) -> bool {
    let mut at = Some(node);
    while let Some(current) = at {
        if current.child_by_field_name(PARAMETERS_FIELD).is_some() {
            return false;
        }
        at = current.parent();
    }
    true
}

/// The simple name of a possibly-generic, possibly-qualified type:
/// `com.acme.Props<String>` → `Props`.
fn simple_type_name(declared: &str) -> &str {
    let head = declared.split(['<', '[']).next().unwrap_or(declared).trim();
    head.rsplit(['.', ':']).next().unwrap_or(head).trim()
}

/// Whether `text` is a single unqualified identifier — the receiver shape
/// [`DeclaredTypes`] can answer for **directly**.
///
/// A `this.client` or a `Holder.INSTANCE` receiver is not one. This predicate
/// refuses both, rather than trimming either down to its last segment: the
/// trimming would resolve `holder.api` against a same-named local, which is a
/// different object.
///
/// The two do not end the same way, and since S-398 the difference is visible
/// one level up rather than here. [`BindingView::receiver_name`] tries this
/// predicate first and falls back to [`self_qualified`], so a receiver qualified
/// by the reading language's **self reference** is reduced after all — that one
/// names a field of the enclosing class and can name nothing else, which is
/// exactly what `holder.api` cannot promise. `Holder.INSTANCE` and `holder.api`
/// stay refused, at this hop, unchanged.
///
/// It is therefore still **narrower than the measurement harness's**
/// `operand_name`, which is deliberately generous and reduces *any* qualified
/// receiver to its last segment before resolving. That is a real difference in
/// answers, not just in shape: the census can still resolve a `holder.api` site
/// the shipped pipeline refuses, and a census figure the product cannot
/// reproduce is the safer direction of the two. What S-398 removed from that gap
/// is the self-qualified shape alone — which is the shape the reference estate
/// actually writes, and is why the gap was worth narrowing there and nowhere
/// else.
///
/// This is also where a **chained** accessor is refused —
/// `config.getMail().getHost()`, the nested-properties-type ceiling each
/// `properties.scm` records — because the receiver's text is then a whole call
/// expression and no file declares a type for that. Stated here rather than
/// guarded separately: a second, earlier "is the receiver a call" test was
/// written, and mutation-testing showed it could not fail, because a call
/// expression's text is never a bare identifier in any grammar.
fn simple_identifier(text: &str) -> Option<&str> {
    let text = text.trim();
    let mut chars = text.chars();
    let first = chars.next()?;
    (first.is_alphabetic() || first == '_')
        .then_some(text)
        .filter(|t| t.chars().all(|c| c.is_alphanumeric() || c == '_'))
}

/// The field name behind a **self-qualified** receiver — `this.api` → `api`
/// (S-398), where `self_reference` is the spelling the reading language declares
/// ([`PropertiesIndex::self_references`](super::binding::PropertiesIndex::self_references)).
///
/// This is the one receiver shape [`simple_identifier`] refuses that is safe to
/// reduce, and the asymmetry is the whole justification. Reducing `holder.api`
/// to `api` resolves against a same-named local — *a different object*, which is
/// why that refusal stands. A self reference names a field of the **enclosing
/// class** and can name nothing else, so the reduction names the same object the
/// source wrote.
///
/// **That is a claim about the source, and it does not on its own survive the
/// lookup** — a correction review forced, after three reviewers reproduced the
/// gap. [`DeclaredTypes`] is file-scoped and scope-blind, so the reduced name
/// can be answered by a local or a parameter rather than by the field; where the
/// field is *inherited* nothing in the file poisons that shadow. The claim is
/// made true by the caller, not here: [`BindingView::declared_receiver_type`]
/// resolves this function's answer through [`DeclaredTypes::field`], which
/// answers from field positions only. What remains is the file-scope residue
/// this module already carries elsewhere — a *sibling class* in the same file
/// declaring the name as its own field answers for it — bounded exactly as the
/// annotation-element ceiling above is, and no wider than the bare arm's own
/// scope-blindness.
///
/// The estate's shape reaches this function and only this function — its
/// declared type was already in [`DeclaredTypes`] (a `private Foo bar;`
/// declarator is exactly what that walk's one-hop rule is written for), and the
/// receiver's **text** was the only thing refusing the site.
///
/// # A whole token, and a separator after it
///
/// - **Whole token**, via `strip_prefix`: the qualifier must begin the text, so
///   a receiver whose head segment merely *ends* with the spelling (`notthis`)
///   is not one. Probed with that near miss, which reaches this function.
/// - **At least one separator after it** — `name.len() < rest.len()`. **No
///   currently declared vocabulary can reach this test, and the honest reason is
///   worth writing down rather than a near miss that does not exist.** The
///   caller tries [`simple_identifier`] first, and a qualifier spelled with
///   identifier characters (Java's `this` is the only one shipped) followed by
///   an identifier is *itself* an identifier — so `thisApi` is claimed by that
///   first arm and never arrives here. The test is retained as the shape's own
///   invariant for a qualifier that is **not** identifier-legal (`$this`), which
///   would reach it; it is not retained on the strength of a probe, and this
///   module deletes guards that cannot fail (see [`simple_identifier`]) rather
///   than dressing them up. The related trap — an identifier character read as a
///   separator — is not defensible in text at all and is gated on the tree
///   instead; see [`BindingView::declared_receiver_type`].
///
/// What survives must still be a single [`simple_identifier`], so `this.a.b`
/// and `this.get().x` are refused at the same hop every other unknown receiver
/// is.
fn self_qualified<'t>(text: &'t str, self_reference: &str) -> Option<&'t str> {
    let rest = text.trim().strip_prefix(self_reference)?;
    let name = rest.trim_start_matches(|c: char| !c.is_alphanumeric() && c != '_');
    (name.len() < rest.len())
        .then(|| simple_identifier(name))
        .flatten()
}

/// The `(receiver, callee-name)` of a member call, or [`None`] for any other
/// operand shape — a bare call with no receiver, a plain name, a literal.
///
/// A `function` fallback for the callee was written here and is deleted: across
/// all 26 vendored grammars **no node type declares both an `object` field and a
/// `function` field**, so it could never contribute an answer — the same
/// standard that removed this function's first receiver guard. A grammar that
/// spells a member call positionally (Kotlin does) is not reached by this
/// function at all; see the module docs' ceiling.
///
/// It makes no judgement about the receiver beyond its existence.
/// [`BindingView::declared_receiver_type`] is where a receiver this module
/// cannot answer for is refused: it accepts a bare identifier, and — since
/// S-398 — a receiver qualified by the reading language's self reference, and
/// refuses everything else, a `Holder.INSTANCE` and a receiver that is itself a
/// call included.
fn member_call<'t>(node: Node<'t>) -> Option<(Node<'t>, Node<'t>)> {
    Some((
        node.child_by_field_name(OBJECT_FIELD)?,
        node.child_by_field_name(NAME_FIELD)?,
    ))
}

/// Everything the invocation arm needs to read one operand as a configuration
/// accessor: the member's class index, the reading file's declared types, and
/// the language whose accessor convention judges the name.
///
/// A struct rather than four more parameters: the capture dispatch it is threaded
/// through is already over clippy's argument limit, and these four are one
/// thing — the answer to "what does this member's configuration say?" — that a
/// second consumer would want whole.
///
/// `module` is the one field that encodes no choice today: the only production
/// constructor of the index stamps [`MEMBER_SCOPE`](super::binding::MEMBER_SCOPE)
/// on every class, so nothing else can be passed meaningfully. It is a field
/// rather than a constant read inside [`key_for`](Self::placeholder_for) so the
/// scope a lookup is made in stays visible at the construction site, next to the
/// index it must agree with.
pub struct BindingView<'a> {
    /// Every configuration-bound class the member declares.
    pub index: &'a PropertiesIndex,
    /// The reading file's `name` → declared simple type bindings.
    pub types: DeclaredTypes,
    /// The plugin name whose accessor convention judges a name
    /// ([`PropertiesIndex::names_an_accessor`]).
    pub language: &'a str,
    /// The module scope the class lookup is made in
    /// ([`PropertiesIndex::get`]).
    pub module: &'a str,
}

impl BindingView<'_> {
    /// The configuration key `operand` names, spelled as the `${…}` placeholder
    /// that carries its **canonical** form — or [`None`] at the first hop the
    /// source does not prove.
    ///
    /// Arm-neutral, and said that way since S-409: this used to open "the request
    /// path", which was true while the HTTP arm was the only caller and became a
    /// shared seam described from one of its two callers. The broker arm asks the
    /// same question of a topic operand ([`crate::extract::broker`]).
    ///
    /// The chain is [FR-WS-19]'s: *accessor → field → owning class → annotation
    /// prefix → canonical key*. The placeholder spelling is produced here, and
    /// not by the caller, because the last step below is to check that the
    /// placeholder **reads back as the key it was built from** — a check that
    /// only means anything where the two are written together.
    ///
    /// [FR-WS-19]: ../../../../docs/specs/requirements/FR-WS-19.md
    pub fn placeholder_for(&self, operand: Node<'_>, src: &[u8]) -> Option<String> {
        let (receiver, callee) = member_call(operand)?;
        // A property getter takes NO arguments, so a call that passes one is not
        // the accessor its name resembles — `kafkaTopics.getArchiveEvents(suffix)`
        // computes something, and binding it to the `archiveEvents` property would
        // fabricate a key the source does not prove ([NFR-RA-05]). Found by the
        // S-409 review; the shape has 0 occurrences on the reference estate, so
        // this tightens both arms against a hazard rather than removing a
        // measured admission. Named children only: the `arguments` node always
        // exists and its parentheses are anonymous.
        if operand
            .child_by_field_name(ARGUMENTS_FIELD)
            .is_some_and(|args| args.named_child_count() > 0)
        {
            return None;
        }
        let accessor = callee.utf8_text(src).ok()?.trim();
        // The shape question, asked in the READING file's own language, and it
        // is a correctness guard rather than a fast path. `bind` below judges by
        // the DECLARING class's language, so a Java file reading a Kotlin-
        // declared class would have its member call judged by Kotlin's
        // convention — and Kotlin declares the empty accessor prefix, under
        // which every name is already a property name. Without this test a Java
        // `k.compute()` would bind to a Kotlin `compute` property: the
        // convention leak `PropertiesIndex::conventions` documents at length,
        // reaching production through the use site instead of through the index.
        //
        // For a same-language read it changes nothing — `bind` returns
        // `NotAnAccessor` for the same names — which is why the guard is stated
        // as the cross-language one it is.
        if !self.index.names_an_accessor(self.language, accessor) {
            return None;
        }
        let declared = self.declared_receiver_type(receiver, src)?;
        let class = self.index.get(declared, self.module)?;
        let binding = self.index.bind(class, accessor).ok()?;
        // Canonical, not the source spelling: this key is recorded for a
        // resolution that canonicalises anyway, and recording the canonical form
        // makes the stored operand key byte-identical for two sites that spell
        // the same property differently.
        placeholder(&canonical_key(&binding.key))
    }

    /// The simple type this file declares for the object `receiver` names — the
    /// one fact the whole chain turns on, and the only hop S-398 changed.
    ///
    /// Two arms, and **each asks a different question of [`DeclaredTypes`]**,
    /// which is the correction three independent reviews forced:
    ///
    /// - A **bare identifier** receiver names whatever the file declares under
    ///   that name, at any position — [`DeclaredTypes::get`]. Unchanged.
    /// - A **self-qualified** receiver names a *field of the enclosing class*,
    ///   so it asks [`DeclaredTypes::field`], which answers from field positions
    ///   only. Asking `get` here read a same-named local or parameter as though
    ///   it were the field, and where the real field was inherited nothing
    ///   poisoned the entry — a reproduced fabrication ([NFR-RA-05]).
    ///
    /// The qualifier is judged in the **reading** file's language, the same
    /// language the accessor's shape is judged in and for the same reason: the
    /// spelling being read is the one this file wrote, not the one the declaring
    /// class's language uses. A language declaring no qualifier reaches only the
    /// first arm, which is the whole of the pre-S-398 behaviour.
    ///
    /// Ordered bare-first so the qualified reading is only ever reached by a
    /// receiver the existing predicate already refused: this widens what is
    /// *captured* and changes no answer that had one.
    ///
    /// # The second arm is gated on the TREE, not on the text
    ///
    /// `receiver.named_child_count() > 0` — the qualified arm is offered only a
    /// receiver the grammar itself parsed as **more than one thing**. Without
    /// it the arm judged by text alone, and text cannot tell a qualifier from a
    /// name: `$` and a zero-width non-joiner are legal *inside* a Java
    /// identifier but are not `is_alphanumeric`, so a receiver spelled
    /// `this$api` — one identifier, which the file may well declare under an
    /// unrelated type — was split into `this` + `api` and bound the wrong
    /// class's key. Two independent reviews reproduced that end to end.
    ///
    /// The grammar has no such ambiguity: `this$api` is a single leaf, and
    /// `this.api` is a node with children. It is also the same leaf test
    /// [`DeclaredTypes::build`] already makes on a name node, and it names no
    /// node kind and no grammar field, so it costs the module nothing it was
    /// keeping ([NFR-MA-01]).
    ///
    /// [NFR-MA-01]: ../../../../docs/specs/requirements/NFR-MA-01.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    fn declared_receiver_type(&self, receiver: Node<'_>, src: &[u8]) -> Option<&str> {
        let text = receiver.utf8_text(src).ok()?;
        if let Some(bare) = simple_identifier(text) {
            return self.types.get(bare);
        }
        if receiver.named_child_count() == 0 {
            return None;
        }
        self.index
            .self_references(self.language)
            .find_map(|spelling| self_qualified(text, spelling))
            .and_then(|field| self.types.field(field))
    }
}

/// `key` as a `${…}` placeholder, but **only if the placeholder reader gets
/// `key` back out of it** ([NFR-RA-05]).
///
/// The key is built from a prefix the annotation spells, and `canonical_key`
/// lowercases and drops `-`/`_` and touches nothing else — so every other byte
/// of that literal survives into a string this arm is about to hand to
/// [`placeholder_keys`](crate::resolve::binding::placeholder_keys). Two of those
/// bytes are load-bearing to that reader and were reproduced doing damage: it
/// stops a key at the first `}`, and it reads the first `:` as the inline-
/// default separator. A prefix of `mail:server.api` therefore yielded the key
/// `mail` — a *different*, shorter key, which a corpus can perfectly well define,
/// so the site bound a value it was never entitled to instead of refusing.
///
/// Expressed as a round-trip through the real reader rather than as a rejected
/// character set, so the two can never disagree about what is a placeholder: if
/// the reader would see anything other than exactly this key, the operand is not
/// proven and resolves to nothing.
///
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
fn placeholder(key: &str) -> Option<String> {
    let spelled = format!("${{{key}}}");
    (crate::resolve::binding::placeholder_keys(&spelled)
        .is_some_and(|keys| keys == [key.to_string()]))
    .then_some(spelled)
}

#[cfg(all(test, feature = "lang-java"))]
#[path = "accessor_tests.rs"]
mod tests;
