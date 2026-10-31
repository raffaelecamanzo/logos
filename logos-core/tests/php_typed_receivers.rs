//! A PHP call on a natively typed receiver binds among its type's methods
//! (S-552, CR-176, FR-RS-27, FR-RS-10, FR-RS-12, NFR-RA-05) — exercised end to
//! end through the public [`Engine`] façade against temp-directory fixtures,
//! with the SHIPPED `references.scm` and `plugin.toml` of the PHP plugin (no
//! query override).
//!
//! What the fixtures pin, over a controller calling the classes it is handed:
//!
//! - a receiver whose type the file proves — a typed parameter (`Mailer $m`,
//!   `?Pager $p`), `$x = new T()`, a typed property or a constructor-promoted
//!   one read through `$this->` — is the Path-form `T::m` a static call
//!   `T::m()` records (no receiver shape), `T` read through the file's `use`
//!   imports and namespace, and binds among `T`'s methods;
//! - a receiver the file proves no type for — an untyped parameter or
//!   property, a union type, a name re-bound by any other assignment, a
//!   closure's own untyped parameter hiding a typed outer one — stays the
//!   Method-form row of shape `other`, which the binder binds nowhere
//!   (`no-receiver-evidence`).
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite (`php_static_calls.rs`): a fixture tree checked into this
//! repository would be indexed into its own graph.

#![cfg(feature = "lang-php")]

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId, ReceiverShape, RefForm};
use logos_core::models::CallResidueReason;
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

#[path = "support/coordinate.rs"]
mod coordinate;

fn tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in files {
        let path = tmp.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    coordinate::pin(tmp.path());
    tmp
}

fn index(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// A node's label: `file:name@line`.
fn labels(rt: &Runtime) -> HashMap<NodeId, String> {
    rt.submit_read(|store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .map(|n| {
                let file = n.file_path.unwrap_or_default();
                (n.id, format!("{file}:{}@{}", n.name, n.start_line.unwrap_or(0)))
            })
            .collect())
    })
    .expect("read runs")
}

/// Bound edges as `(source label, target label)`.
type Edges = Vec<(String, String)>;

/// Every bound `Calls` edge as `(source label, target label)`, sorted.
fn calls(rt: &Runtime) -> Edges {
    let label = labels(rt);
    let mut out: Edges = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .map(|e| (label[&e.source].clone(), label[&e.target].clone()))
        .collect();
    out.sort();
    out
}

/// One ledger row: `(source label, target, form, receiver, resolved)`.
type Row = (String, String, RefForm, Option<ReceiverShape>, bool);

/// The `Calls` rows sourced in `file` whose target ends in one of `names`,
/// sorted.
fn rows(rt: &Runtime, file: &str, names: &[&str]) -> Vec<Row> {
    let label = labels(rt);
    let by_symbol: HashMap<String, String> = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .map(|n| (n.symbol.as_str().to_string(), label[&n.id].clone()))
                .collect())
        })
        .expect("read runs");
    let prefix = format!("{file}:");
    let rows: Vec<Row> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Calls && r.form != RefForm::Symbol)
        .filter(|r| names.iter().any(|n| r.target == *n || r.target.ends_with(&format!("::{n}"))))
        .filter_map(|r| {
            let label = by_symbol.get(&r.source_symbol)?;
            label
                .starts_with(&prefix)
                .then(|| (label.clone(), r.target, r.form, r.receiver, r.resolved))
        })
        .collect();
    sorted(rows)
}

/// `rows` sorted by `(source, target)`.
fn sorted(mut rows: Vec<Row>) -> Vec<Row> {
    rows.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    rows
}

fn edge(from: &str, to: &str) -> (String, String) {
    (from.to_string(), to.to_string())
}

const OTHER: Option<ReceiverShape> = Some(ReceiverShape::Other);

/// A typed row: the Path `T::m` a static call records, bound.
fn typed(source: String, target: &str) -> Row {
    (source, target.to_string(), RefForm::Path, None, true)
}

/// An untyped row: the Method-form `m` of shape `other`, bound nowhere.
fn untyped(source: String, target: &str) -> Row {
    (source, target.to_string(), RefForm::Method, OTHER, false)
}

const MAIL: &str = "\
<?php
namespace App\\Mail;

class Mailer
{
    public function send() {}
    public function queue() {}
}

class SpecialMailer extends Mailer {}
";

/// A second class declaring `send`: a call bound by its name alone could reach
/// either, so every edge into one of them is the receiver's type at work.
const PAGE: &str = "\
<?php
namespace App\\Mail;

class Pager
{
    public function send() {}
}
";

const CONTROLLER: &str = "\
<?php
namespace App\\Http;

use App\\Mail\\Mailer;
use App\\Mail\\SpecialMailer;
use App\\Mail\\Pager;

class Controller
{
    private Mailer $mailer;
    private ?Pager $pager;
    private $loose;
    public function __construct(private Mailer $promoted, protected readonly ?Pager $maybe, $plain) { $promoted->queue(); $maybe->send(); }
    public function byParam(Mailer $m) { $m->send(); }
    public function byNullable(?Pager $p) { $p->send(); }
    public function byNew() { $made = new Mailer(); $made->queue(); }
    public function byField() { $this->mailer->send(); }
    public function byNullableField() { $this->pager->send(); }
    public function byPromoted() { $this->promoted->send(); }
    public function byNullablePromoted() { $this->maybe->send(); }
    public function byBase(SpecialMailer $s) { $s->queue(); }
    public function byArrow(Mailer $a) { return fn() => $a->send(); }
    public function untyped($x) { $x->send(); }
    public function untypedProperty() { $this->loose->send(); }
    public function reassigned(Mailer $r) { $r = make(); $r->send(); }
    public function shadowed(Mailer $c) { return function ($c) { $c->send(); }; }
    public function union(Mailer|Pager $u) { $u->send(); }
}

function f(Mailer $m) { $m->send(); }
";

const NAMES: &[&str] = &["send", "queue"];

#[test]
fn a_call_on_a_natively_typed_receiver_binds_among_its_types_methods() {
    let tmp = tree(&[
        ("src/Mail/Mailer.php", MAIL),
        ("src/Mail/Pager.php", PAGE),
        ("src/Http/Controller.php", CONTROLLER),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let c = |line: u32, name: &str| format!("src/Http/Controller.php:{name}@{line}");
    assert_eq!(
        rows(rt, "src/Http/Controller.php", NAMES),
        sorted(vec![
            typed(c(13, "__construct"), "Mailer::queue"),
            typed(c(13, "__construct"), "Pager::send"),
            typed(c(17, "byField"), "Mailer::send"),
            typed(c(22, "byArrow"), "Mailer::send"),
            typed(c(21, "byBase"), "SpecialMailer::queue"),
            typed(c(16, "byNew"), "Mailer::queue"),
            typed(c(18, "byNullableField"), "Pager::send"),
            typed(c(20, "byNullablePromoted"), "Pager::send"),
            typed(c(15, "byNullable"), "Pager::send"),
            typed(c(14, "byParam"), "Mailer::send"),
            typed(c(19, "byPromoted"), "Mailer::send"),
            untyped(c(25, "reassigned"), "send"),
            untyped(c(26, "shadowed"), "send"),
            untyped(c(27, "union"), "send"),
            untyped(c(24, "untypedProperty"), "send"),
            untyped(c(23, "untyped"), "send"),
            typed(c(30, "f"), "Mailer::send"),
        ])
    );
    let edges = calls(rt);
    let send = "src/Mail/Mailer.php:send@6";
    let queue = "src/Mail/Mailer.php:queue@7";
    let pager_send = "src/Mail/Pager.php:send@6";
    for expected in [
        edge(&c(13, "__construct"), queue),
        edge(&c(13, "__construct"), pager_send),
        edge(&c(14, "byParam"), send),
        edge(&c(15, "byNullable"), pager_send),
        edge(&c(16, "byNew"), queue),
        edge(&c(17, "byField"), send),
        edge(&c(18, "byNullableField"), pager_send),
        edge(&c(19, "byPromoted"), send),
        edge(&c(20, "byNullablePromoted"), pager_send),
        // Inheritance: `SpecialMailer` declares no `queue`; its base does.
        edge(&c(21, "byBase"), queue),
        edge(&c(22, "byArrow"), send),
        edge(&c(30, "f"), send),
    ] {
        assert!(edges.contains(&expected), "missing {expected:?} in {edges:#?}");
    }
    // Nothing the file proves no type for reaches either `send`.
    for caller in ["untyped@", "untypedProperty@", "reassigned@", "shadowed@", "union@"] {
        assert!(!edges.iter().any(|(from, _)| from.contains(caller)), "{caller}: {edges:#?}");
    }
}

#[test]
fn impact_of_the_called_method_lists_every_typed_caller() {
    let tmp = tree(&[
        ("src/Mail/Mailer.php", MAIL),
        ("src/Mail/Pager.php", PAGE),
        ("src/Http/Controller.php", CONTROLLER),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let callers: Vec<String> = calls(rt)
        .into_iter()
        .filter(|(_, to)| to == "src/Mail/Pager.php:send@6")
        .map(|(from, _)| from)
        .collect();
    assert_eq!(
        callers,
        vec![
            "src/Http/Controller.php:__construct@13".to_string(),
            "src/Http/Controller.php:byNullable@15".to_string(),
            "src/Http/Controller.php:byNullableField@18".to_string(),
            "src/Http/Controller.php:byNullablePromoted@20".to_string(),
        ]
    );
}

/// Index `initial`, overwrite `edits` and sync them; then index the post-edit
/// tree from scratch. The two graphs must be identical (NFR-RA-06); the synced
/// one's `Calls` edges are returned.
fn synced_equals_reindexed(initial: &[(&str, &str)], edits: &[(&str, &str)]) -> Edges {
    let tmp_a = tree(initial);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    let mut changed: Vec<PathBuf> = Vec::new();
    for (rel, text) in edits {
        let path = tmp_a.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
        changed.push(root_a.join(rel));
    }
    engine_a.sync(&changed);
    let rt_a = engine_a.runtime().unwrap();

    let mut final_state: BTreeMap<&str, &str> = initial.iter().copied().collect();
    final_state.extend(edits.iter().copied());
    let files: Vec<(&str, &str)> = final_state.into_iter().collect();
    let tmp_b = tree(&files);
    let engine_b = index(tmp_b.path());

    assert_eq!(
        graph_fingerprint(rt_a),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    calls(rt_a)
}

const SMALL_CALLER: &str = "\
<?php
namespace App\\Http;

use App\\Mail\\Mailer;

class Small
{
    public function run(Mailer $m) { $m->send(); }
}
";

#[test]
fn a_synced_edit_to_the_receivers_type_matches_a_fresh_reindex() {
    let initial = [
        ("src/Http/Small.php", SMALL_CALLER),
        ("src/Mail/Mailer.php", MAIL),
        ("src/Mail/Pager.php", PAGE),
    ];
    // The parameter's declared type changes: the row is re-typed and re-bound.
    let retyped = SMALL_CALLER.replace("use App\\Mail\\Mailer;", "use App\\Mail\\Pager;").replace("Mailer $m", "Pager $m");
    let edges = synced_equals_reindexed(&initial, &[("src/Http/Small.php", &retyped)]);
    assert_eq!(edges, vec![edge("src/Http/Small.php:run@8", "src/Mail/Pager.php:send@6")]);
    // The called type loses its method: the typed row stays unbound.
    let gutted = PAGE.replace("public function send() {}", "");
    let edges = synced_equals_reindexed(&initial, &[("src/Http/Small.php", &retyped), ("src/Mail/Pager.php", &gutted)]);
    assert_eq!(edges, Vec::<(String, String)>::new());
}

// ── What the file does not prove ────────────────────────────────────────────

/// Each method's `$m` is a typed parameter that a second binding hides — or a
/// declaration whose type proves nothing. `Hidden` declares a `send` of its
/// own, which a `self`-typed receiver would reach if it were proven. `Loose`'s
/// untyped properties share their names with `Typed`'s typed ones: the file
/// declares each name with no type a call is typed by, so neither is proven.
/// `Selfish` declares its own `send`, which a `self`- or `static`-typed
/// property, promoted parameter or `new static()` would reach if it were proven.
const HIDDEN: &str = "\
<?php
namespace App\\Http;

use App\\Mail\\Mailer;

class Hidden
{
    public function send() {}
    public function assigned(Mailer $m) { $m = make(); $m->send(); }
    public function referenced(Mailer $m, $o) { $m = &$o; $m->send(); }
    public function augmented(Mailer $m) { $m ??= make(); $m->send(); }
    public function listed(Mailer $m) { [$m, $n] = make(); $m->send(); }
    public function looped(Mailer $m, $xs) { foreach ($xs as $m) { $m->send(); } }
    public function keyed(Mailer $m, $xs) { foreach ($xs as $m => $v) { $m->send(); } }
    public function byReference(Mailer $m, $xs) { foreach ($xs as &$m) { $m->send(); } }
    public function unpacked(Mailer $m, $xs) { foreach ($xs as [$m]) { $m->send(); } }
    public function caught(Mailer $m) { try { go(); } catch (\\Exception $m) { $m->send(); } }
    public function globals(Mailer $m) { global $m; $m->send(); }
    public function statics(Mailer $m) { static $m; $m->send(); }
    public function closure(Mailer $m) { return function ($m) { $m->send(); }; }
    public function arrow(Mailer $m) { return fn($m) => $m->send(); }
    public function variadic(Mailer $m) { return function (Mailer ...$m) { $m->send(); }; }
    public function qualified(\\App\\Mail\\Mailer $m) { $m->send(); }
    public function own(self $m) { $m->send(); }
    public function made() { $m = new self(); $m->send(); }
    public function primitive(?int $m) { $m->send(); }
}

class Typed
{
    private Mailer $svc;
    public function __construct(private Mailer $dep) {}
}

class Loose
{
    private $svc;
    public function __construct(private $dep) {}
    public function run() { $this->svc->send(); }
    public function promoted() { $this->dep->send(); }
}

class Selfish
{
    private self $me;
    public function __construct(private self $peer) { $peer->send(); }
    public function send() {}
    public function viaMe() { $this->me->send(); }
    public function viaPeer() { $this->peer->send(); }
    public function made() { $m = new static(); $m->send(); }
    public function parental() { $m = new parent(); $m->send(); }
}

function make() {}
function go() {}
";

#[test]
fn a_receiver_the_file_proves_no_type_for_binds_nowhere() {
    let tmp = tree(&[("src/Mail/Mailer.php", MAIL), ("src/Http/Hidden.php", HIDDEN)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let typed: Vec<Row> = rows(rt, "src/Http/Hidden.php", &["send"])
        .into_iter()
        .filter(|r| r.2 != RefForm::Method || r.3 != OTHER || r.4)
        .collect();
    assert_eq!(typed, Vec::<Row>::new());
    // Each is classified `no-receiver-evidence`; the file's free calls bind.
    let unproven = rows(rt, "src/Http/Hidden.php", &["send"]).len() as u64;
    let residue = engine
        .status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == "php")
        .and_then(|row| row.call_residue)
        .expect("the php row states its call residue");
    assert_eq!(residue.reasons.get(&CallResidueReason::NoReceiverEvidence), Some(&unproven));
    assert_eq!(unproven, 25);
    let reached: Edges = calls(rt).into_iter().filter(|(_, to)| to.contains(":send@")).collect();
    assert_eq!(reached, Edges::new());
}

// ── Every declaration form that proves a type ───────────────────────────────

const FORMS: &str = "\
<?php
namespace App\\Http;

use App\\Mail\\Mailer;

class Forms
{
    public function __construct(private Mailer &$held) { $held->send(); }
    public function byReference(Mailer &$m) { $m->send(); }
    public function byDefault(?Mailer $m = null) { $m->send(); }
    public function bareNew() { $m = new Mailer; $m->send(); }
    public function byHeld() { $this->held->send(); }
    public function inClosure(Mailer $m) { return function () use ($m) { $m->send(); }; }
    public function siblings() { $a = function (Mailer $m) { $m->send(); }; $b = function ($m) { return $m; }; }
}

$top = new Mailer();
$top->queue();
";

#[test]
fn every_declaration_form_php_writes_proves_its_type() {
    let tmp = tree(&[("src/Mail/Mailer.php", MAIL), ("src/Http/Forms.php", FORMS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let f = |line: u32, name: &str| format!("src/Http/Forms.php:{name}@{line}");
    let send = "src/Mail/Mailer.php:send@6";
    let from_forms: Edges = calls(rt).into_iter().filter(|(from, _)| from.starts_with("src/Http/Forms.php:")).collect();
    assert_eq!(
        from_forms,
        vec![
            // Script code outside every callable: the file's own binding.
            edge(&f(1, "Forms"), "src/Mail/Mailer.php:queue@7"),
            edge(&f(8, "__construct"), send),
            edge(&f(11, "bareNew"), send),
            edge(&f(10, "byDefault"), send),
            edge(&f(12, "byHeld"), send),
            edge(&f(9, "byReference"), send),
            edge(&f(13, "inClosure"), send),
            // A parameter is in scope in its own closure only: the sibling's
            // untyped `$m` hides nothing here.
            edge(&f(14, "siblings"), send),
        ]
    );
}

// ── `T` is read through every `use` form ────────────────────────────────────

/// A class of the caller's own namespace sharing the imported one's name: a
/// typed row read through any scope but the import binds to it.
const LOCAL_MAILER: &str = "\
<?php
namespace App\\Http;

class Mailer
{
    public function send() {}
}
";

const GLOBAL_MAILER: &str = "\
<?php
class Mailer
{
    public function send() {}
}
";

/// A grouped `use` and an alias, inside a group or not, bind the class they
/// import — never the caller's namespace's same-named class, nor the global one.
const GROUPED: &str = "\
<?php
namespace App\\Http;

use App\\Mail\\{Mailer, SpecialMailer};

class Grouped
{
    public function run(Mailer $m) { $m->send(); }
    public function base(SpecialMailer $s) { $s->queue(); }
}
";

const GROUPED_ALIAS: &str = "\
<?php
namespace App\\Http;

use App\\Mail\\{Pager as Mailer};

class GroupedAlias
{
    public function run(Mailer $m) { $m->send(); }
}
";

const ALIASED: &str = "\
<?php
namespace App\\Http;

use App\\Mail\\Pager as Mailer;

class Aliased
{
    public function run(Mailer $m) { $m->send(); }
}
";

#[test]
fn a_typed_receivers_type_is_read_through_every_use_form() {
    let tmp = tree(&[
        ("src/Mail/Mailer.php", MAIL),
        ("src/Mail/Pager.php", PAGE),
        ("src/Http/Mailer.php", LOCAL_MAILER),
        ("src/Global.php", GLOBAL_MAILER),
        ("src/Http/Grouped.php", GROUPED),
        ("src/Http/GroupedAlias.php", GROUPED_ALIAS),
        ("src/Http/Aliased.php", ALIASED),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let typed_callers: Edges = calls(rt)
        .into_iter()
        .filter(|(from, _)| ["Grouped.php:", "GroupedAlias.php:", "Aliased.php:"].iter().any(|f| from.contains(f)))
        .collect();
    assert_eq!(
        typed_callers,
        vec![
            edge("src/Http/Aliased.php:run@8", "src/Mail/Pager.php:send@6"),
            edge("src/Http/Grouped.php:base@9", "src/Mail/Mailer.php:queue@7"),
            edge("src/Http/Grouped.php:run@8", "src/Mail/Mailer.php:send@6"),
            edge("src/Http/GroupedAlias.php:run@8", "src/Mail/Pager.php:send@6"),
        ]
    );
    // An `as` name is the import's alias, never an import of its own.
    let imports: Vec<(String, Option<String>)> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Imports && r.source_symbol.contains("`Aliased.php`"))
        .map(|r| (r.target, r.alias))
        .collect();
    assert_eq!(imports, vec![("App::Mail::Pager".to_string(), Some("Mailer".to_string()))]);
}
