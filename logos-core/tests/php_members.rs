//! PHP promoted properties, class constants and enum cases are symbols (S-548,
//! CR-176, FR-EX-21, FR-EX-02, NFR-RA-05, NFR-RA-06) — exercised end to end
//! through the public [`Engine`] façade against temp-directory fixtures, with
//! the SHIPPED `symbols.scm` of the PHP plugin (no query override).
//!
//! What the fixtures pin:
//!
//! - a constructor-promoted parameter (`public function __construct(private
//!   Logger $log)`) is a `Field` owned by the **class**, not by the constructor
//!   it is written in, and `$this->log` in a sibling method binds to it
//!   (`Accesses`, Method → Field) — the shape of monolog's `LogRecord`. A
//!   by-reference promotion (`&$rows`) is one too; a plain parameter is none, and
//!   so is a promotion outside a named class's or trait's constructor (an
//!   anonymous class's, a function's, a closure's), which no class owns;
//! - a class, interface, trait or enum constant is a `Constant` under its type,
//!   each element of `const A = 1, B = 2;` its own node; an enum case is a
//!   `Constant` under its enum; a global `const`, an anonymous class's constant
//!   and a constant's *value* name (`const A = B;`) are none;
//! - a constant sharing a name with a property does not move the property's
//!   symbol (the constant numbers after what the graph already held);
//! - the new symbols read their `exported` flag from the modifier of the
//!   declaration that holds them (`private`/`protected` demote, as for a method);
//! - every symbol the file held before is byte-identical, and a synced edit
//!   equals a fresh reindex.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite (`php_static_calls.rs`): a fixture tree checked into this
//! repository would be indexed into its own graph.

#![cfg(feature = "lang-php")]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId, NodeKind};
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

/// Every node as `(kind, symbol descriptors under the fixture coordinate)`.
fn symbols(rt: &Runtime) -> BTreeSet<(String, String)> {
    let prefix = coordinate::symbol("");
    rt.submit_read(|store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.kind.as_str().to_string(), n.symbol.as_str().trim_start_matches(&prefix).to_string()))
            .collect())
    })
    .expect("read runs")
}

/// The descriptors of every node of `kind`, sorted.
fn of_kind(rt: &Runtime, kind: NodeKind) -> Vec<String> {
    symbols(rt)
        .into_iter()
        .filter(|(k, _)| k == kind.as_str())
        .map(|(_, s)| s)
        .collect()
}

/// Every `kind` edge as `(source descriptors, target descriptors)`, sorted.
fn edges(rt: &Runtime, kind: EdgeKind) -> Vec<(String, String)> {
    let prefix = coordinate::symbol("");
    let by_id: HashMap<NodeId, String> = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .map(|n| (n.id, n.symbol.as_str().trim_start_matches(&prefix).to_string()))
                .collect())
        })
        .expect("read runs");
    let mut out: Vec<(String, String)> = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| (by_id[&e.source].clone(), by_id[&e.target].clone()))
        .collect();
    out.sort();
    out
}

fn edge(from: &str, to: &str) -> (String, String) {
    (from.to_string(), to.to_string())
}

/// `LogRecord`'s shape: promoted properties of every visibility (two with
/// defaults, one `readonly`, one by reference), read through `$this->…` in
/// sibling methods, beside a plain parameter that is no property.
const LOG_RECORD: &str = "\
<?php
namespace Mono;

class LogRecord {
    public function __construct(
        public readonly string $channel,
        private array &$rows,
        protected int $level = 0,
        private ?array $extra = null,
        $plain = 1,
    ) {
    }

    public function channel() { return $this->channel; }
    public function bump() { $this->level = $this->level + 1; return $this->extra; }
    public function plain() { return $this->plain; }
    public function rows() { return $this->rows; }
}
";

#[test]
fn a_promoted_parameter_is_a_field_owned_by_the_class_and_its_accesses_bind() {
    let tmp = tree(&[("src/LogRecord.php", LOG_RECORD)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();

    assert_eq!(
        of_kind(rt, NodeKind::Field),
        [
            "src/`LogRecord.php`/LogRecord#channel.",
            "src/`LogRecord.php`/LogRecord#extra.",
            "src/`LogRecord.php`/LogRecord#level.",
            "src/`LogRecord.php`/LogRecord#rows.",
        ]
        .map(String::from),
        "the four promoted parameters (one by reference) are Fields of the class; a plain parameter is none"
    );

    let contains = edges(rt, EdgeKind::Contains);
    for field in ["channel", "extra", "level", "rows"] {
        let to = format!("src/`LogRecord.php`/LogRecord#{field}.");
        assert!(
            contains.contains(&edge("src/`LogRecord.php`/LogRecord#", &to)),
            "the class contains its promoted {field}: {contains:?}"
        );
        assert!(
            !contains.iter().any(|(from, t)| *t == to && from.ends_with("__construct().")),
            "the constructor does not own {field}: {contains:?}"
        );
    }

    let accesses = edges(rt, EdgeKind::Accesses);
    let m = "src/`LogRecord.php`/LogRecord#";
    assert_eq!(
        accesses,
        vec![
            edge(&format!("{m}bump()."), &format!("{m}extra.")),
            edge(&format!("{m}bump()."), &format!("{m}level.")),
            edge(&format!("{m}channel()."), &format!("{m}channel.")),
            edge(&format!("{m}rows()."), &format!("{m}rows.")),
        ],
        "each `$this->x` on a promoted property binds exactly one Field; `$this->plain` names none"
    );
}

const CONSTANTS: &str = "\
<?php
namespace Mono;

class Level {
    const DEBUG = 100;
    public const INFO = 200, NOTICE = 250;
    private const SECRET = self::DEBUG;
    const ALIAS = DEBUG_LEVEL;
    final protected const int TYPED = 1;
}

interface Named {
    const NAME = 'n';
}

trait Tagged {
    const TAG = 't';
}

enum Suit: string {
    case Hearts = 'H';
    case Spades = 'S';
    const Wild = self::Spades;
    public function label() { return $this->value; }
}

const GLOBAL_ONE = 1;

function make() {
    return new class { const INNER = 1; };
}
";

#[test]
fn class_interface_trait_and_enum_constants_and_enum_cases_are_constants_under_their_type() {
    let tmp = tree(&[("src/Level.php", CONSTANTS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let p = "src/`Level.php`/";

    assert_eq!(
        of_kind(rt, NodeKind::Constant),
        [
            "Level#ALIAS.",
            "Level#DEBUG.",
            "Level#INFO.",
            "Level#NOTICE.",
            "Level#SECRET.",
            "Level#TYPED.",
            "Named#NAME.",
            "Suit#Hearts.",
            "Suit#Spades.",
            "Suit#Wild.",
            "Tagged#TAG.",
        ]
        .map(|s| format!("{p}{s}")),
        "every named type's constants and the enum cases; no global const, no anonymous class's, \
         no value name (`DEBUG_LEVEL`, `self::DEBUG`)"
    );

    let contains = edges(rt, EdgeKind::Contains);
    for (owner, member) in [
        ("Level#", "Level#TYPED."),
        ("Level#", "Level#NOTICE."),
        ("Named#", "Named#NAME."),
        ("Tagged#", "Tagged#TAG."),
        ("Suit#", "Suit#Hearts."),
        ("Suit#", "Suit#Wild."),
    ] {
        assert!(
            contains.contains(&edge(&format!("{p}{owner}"), &format!("{p}{member}"))),
            "{owner} contains {member}: {contains:?}"
        );
    }
}

#[test]
fn a_constant_sharing_a_property_name_leaves_the_propertys_symbol_where_it_was() {
    // `const name` precedes `$name` and both render the term descriptor `name.`:
    // the property keeps ordinal 0, the constant is numbered after it.
    let src = "<?php\nclass K {\n    const name = 1;\n    public $name = 2;\n    public function m() { return $this->name; }\n}\n";
    let tmp = tree(&[("K.php", src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();

    assert_eq!(of_kind(rt, NodeKind::Field), ["`K.php`/K#name."].map(String::from));
    assert_eq!(of_kind(rt, NodeKind::Constant), ["`K.php`/K#name.1:"].map(String::from));
}

const PROMOTIONS: &str = "\
<?php
trait Loggable {
    public function __construct(private int $level) {}
    public function level() { return $this->level; }
}

class Outer {
    public function make() {
        return new class(1) {
            public function __construct(private int $inner) {}
        };
    }
}

$anon = new class(1) {
    public function __construct(private int $top) {}
};

function free(public int $a) {}
$closure = function (private $b) {};
$arrow = fn (public $c) => 1;
";

#[test]
fn a_promotion_is_a_field_only_in_a_named_classs_or_traits_constructor() {
    let tmp = tree(&[("src/p.php", PROMOTIONS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();

    assert_eq!(
        of_kind(rt, NodeKind::Field),
        ["src/`p.php`/Loggable#level."].map(String::from),
        "a trait's promoted property is a Field of the trait; an anonymous class's, a function's \
         and a closure's are no class's, so none"
    );
    assert_eq!(
        edges(rt, EdgeKind::Accesses),
        vec![edge("src/`p.php`/Loggable#level().", "src/`p.php`/Loggable#level.")],
        "`$this->level` in the trait binds to the promoted property"
    );
}

/// The `exported` flag of every node named in `names`, as `(name, exported)`.
fn exported_of(rt: &Runtime, names: &[&str]) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = rt
        .submit_read(|store| store.annotation_nodes())
        .expect("read runs")
        .into_iter()
        .filter(|n| names.contains(&n.name.as_str()))
        .map(|n| (n.name, n.exported))
        .collect();
    out.sort();
    out
}

#[test]
fn the_new_symbols_are_exported_unless_their_declaration_is_private_or_protected() {
    let src = "<?php\n\
        class V {\n\
            public const PUB = 1;\n\
            const BARE = 1;\n\
            protected const PROT = 1;\n\
            final private const PRIV = 1;\n\
            public function __construct(\n\
                public int $pubField,\n\
                protected int $protField,\n\
                #[Sensitive] private int $privField,\n\
                private array &$privRef,\n\
                public array &$pubRef,\n\
            ) {}\n\
        }\n\
        enum E { case On; private const HIDDEN = 1; }\n";
    let tmp = tree(&[("V.php", src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();

    let names = [
        "PUB", "BARE", "PROT", "PRIV", "pubField", "protField", "privField", "privRef", "pubRef", "On",
        "HIDDEN",
    ];
    let want = [
        ("BARE", true),
        ("HIDDEN", false),
        ("On", true),
        ("PRIV", false),
        ("PROT", false),
        ("PUB", true),
        ("privField", false),
        ("privRef", false),
        ("protField", false),
        ("pubField", true),
        ("pubRef", true),
    ];
    assert_eq!(
        exported_of(rt, &names),
        want.map(|(n, e)| (n.to_string(), e)),
        "a constant and a promoted property take their declaration's visibility; an enum case is public"
    );
}

/// Existing declarations of every kind the PHP query already captured, beside
/// the constructs this story adds.
const BEFORE: &str = "\
<?php
namespace App;

interface Shape { public function area(); }
trait Loud { public function shout() {} }

class Circle implements Shape {
    use Loud;
    public int $r = 1;
    private static $count = 0;
    public function __construct($seed = 0) { $this->r = $seed; }
    public function area() { return $this->r; }
}

enum Mode { public function flag() {} }

function helper() { return 1; }
";

const AFTER: &str = "\
<?php
namespace App;

interface Shape { const PI = 3; public function area(); }
trait Loud { public function shout() {} }

class Circle implements Shape {
    use Loud;
    const UNIT = 'cm';
    public int $r = 1;
    private static $count = 0;
    public function __construct(private Logger $log, $seed = 0) { $this->r = $seed; }
    public function area() { return $this->r; }
}

enum Mode { case On; public function flag() {} }

function helper() { return 1; }
";

#[test]
fn every_symbol_the_file_held_before_is_unchanged_and_only_the_new_ones_are_added() {
    let before = {
        let tmp = tree(&[("src/a.php", BEFORE)]);
        let engine = index(tmp.path());
        symbols(engine.runtime().unwrap())
    };
    let after = {
        let tmp = tree(&[("src/a.php", AFTER)]);
        let engine = index(tmp.path());
        symbols(engine.runtime().unwrap())
    };

    assert!(
        before.is_subset(&after),
        "a symbol the file held before is gone or renamed: {:?}",
        before.difference(&after).collect::<Vec<_>>()
    );
    let added: Vec<(String, String)> = after.difference(&before).cloned().collect();
    let p = "src/`a.php`/";
    assert_eq!(
        added,
        vec![
            ("constant".to_string(), format!("{p}Circle#UNIT.")),
            ("constant".to_string(), format!("{p}Mode#On.")),
            ("constant".to_string(), format!("{p}Shape#PI.")),
            ("field".to_string(), format!("{p}Circle#log.")),
        ],
        "exactly the promoted property, the class constant, the enum case and the interface constant"
    );
}

/// Index `initial`, overwrite `edits` and sync them; then index the post-edit
/// tree from scratch. The two graphs must be identical (NFR-RA-06).
fn synced_equals_reindexed(initial: &[(&str, &str)], edits: &[(&str, &str)]) -> Vec<(String, String)> {
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
    edges(rt_a, EdgeKind::Accesses)
}

#[test]
fn a_synced_edit_that_adds_a_promoted_property_matches_a_fresh_reindex() {
    let without = LOG_RECORD.replace("public readonly string $channel,", "string $channel,");
    let accesses = synced_equals_reindexed(
        &[("src/LogRecord.php", &without)],
        &[("src/LogRecord.php", LOG_RECORD)],
    );
    assert!(
        accesses.iter().any(|(_, to)| to.ends_with("LogRecord#channel.")),
        "the edit's promoted property is bound after the sync: {accesses:?}"
    );
}
