//! Cohesion and Focus count **bodied** methods (S-502, [CR-163] §3.2 C/D,
//! metric-semantics v7), driven end-to-end through the public [`Engine`] façade
//! against real Java, Rust, Go and C++ fixtures, so the `has_body` fact the
//! dimensions read is the one extraction records ([FR-EX-11]), not a hand-set
//! row.
//!
//! - A MapStruct-style abstract class — 17 bodyless `abstract` declarations
//!   beside 6 bodied helpers, 106 lines — scores LCOM4 over the 6 only and is
//!   not a Focus god container ([FR-QM-11], [FR-QM-12]).
//! - A container with 25 bodied methods is still god ([FR-QM-12]'s 25-method
//!   AC, held for bodied methods). On Java this holds end to end. On Rust and
//!   Go it holds at the metric level (`metrics/tests.rs`, `Struct` rows) but
//!   **not** end to end: extraction makes the crate/package module, not the
//!   struct, the `Contains` parent of an `impl` method or a receiver method, so
//!   a Rust/Go container never has a method count and is god by span alone.
//!   That gap predates CR-163; the two Rust/Go tests below carry the AC's
//!   expectation and stay `#[ignore]`d, naming the gap, until extraction
//!   attaches those methods to their type.
//! - A C++ class whose members are all defined out of line has only bodyless
//!   in-class prototypes (out-of-line definitions are not captured): it is
//!   unscoreable for Cohesion and Focus counts it god only by span — the S-502
//!   "score what remains" decision.
//!
//! [CR-163]: ../../docs/requests/CR-163-structural-metrics-stop-misfiring-on-declarative-code.md
//! [FR-EX-11]: ../../docs/specs/requirements/FR-EX-11.md
//! [FR-QM-11]: ../../docs/specs/requirements/FR-QM-11.md
//! [FR-QM-12]: ../../docs/specs/requirements/FR-QM-12.md

use std::fs;
use std::path::Path;

use logos_core::metrics::{self, RecordedSnapshot};
use logos_core::{Engine, Granularity};
use tempfile::TempDir;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Index `root` and record one snapshot under the default thresholds.
fn scan(root: &Path) -> RecordedSnapshot {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let view = engine
        .hydrate(Granularity::ExcludeContains)
        .expect("dependency view hydrates");
    let rt = engine.runtime().expect("runtime");
    metrics::snapshot(rt, &view, None, metrics::Thresholds::default()).expect("snapshot runs")
}

/// The names the Focus offender list reports (the god containers).
fn god_names(s: &RecordedSnapshot) -> Vec<&str> {
    s.worst_offenders
        .focus
        .iter()
        .map(|o| o.name.as_str())
        .collect()
}

/// The MapStruct-style mapper of [CR-163] §2, exactly 106 lines from
/// `public abstract class` to its closing brace: two fields, 17 `abstract`
/// declarations and 6 bodied helpers. Helpers `a*` share `clock`, helpers `b*`
/// share `zone`, so LCOM4 over the bodied six is 2.
///
/// [CR-163]: ../../docs/requests/CR-163-structural-metrics-stop-misfiring-on-declarative-code.md
fn mapstruct_mapper() -> String {
    let mut class = vec![
        "public abstract class MailboxMapper {".to_string(),
        "    protected Clock clock;".to_string(),
        "    protected ZoneId zone;".to_string(),
    ];
    for i in 0..17 {
        class.push(format!("    public abstract MailboxDto toDto{i}(Mailbox source);"));
    }
    for (name, field) in [
        ("a0", "clock"),
        ("a1", "clock"),
        ("a2", "clock"),
        ("b0", "zone"),
        ("b1", "zone"),
        ("b2", "zone"),
    ] {
        class.push(format!("    protected Object {name}() {{"));
        class.push(format!("        return this.{field};"));
        class.push("    }".to_string());
    }
    // Pad with comment lines so the class spans the CR's 106 lines.
    while class.len() < 105 {
        class.push("    // mapping configuration".to_string());
    }
    class.push("}".to_string());
    assert_eq!(class.len(), 106, "the mapper spans 106 lines");
    format!(
        "package mail;\n\nimport java.time.Clock;\nimport java.time.ZoneId;\n\n{}\n",
        class.join("\n")
    )
}

/// A Java class with `n` bodied methods.
fn java_class(name: &str, n: usize) -> String {
    let mut src = format!("package mail;\n\npublic class {name} {{\n    private int x;\n");
    for i in 0..n {
        src.push_str(&format!(
            "    public int work{i}() {{\n        return this.x + {i};\n    }}\n"
        ));
    }
    src.push_str("}\n");
    src
}

/// [CR-163] §3.2 C/D on Java: the mapper's 17 bodyless declarations leave both
/// dimensions — LCOM4 over the 6 bodied helpers is 2, so the Cohesion offender
/// reads `LCOM4 2` (not 19) — and the mapper is no god container, while a
/// class with 25 bodied methods is.
///
/// [CR-163]: ../../docs/requests/CR-163-structural-metrics-stop-misfiring-on-declarative-code.md
#[test]
fn java_mapstruct_mapper_is_scored_over_its_bodied_methods() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/main/java/mail/MailboxMapper.java",
        &mapstruct_mapper(),
    );
    write(
        tmp.path(),
        "src/main/java/mail/Service.java",
        &java_class("Service", 25),
    );
    let s = scan(tmp.path());

    let mapper: Vec<_> = s
        .worst_offenders
        .cohesion
        .iter()
        .filter(|o| o.name == "MailboxMapper")
        .collect();
    assert_eq!(mapper.len(), 1, "the mapper is a low-cohesion class");
    assert_eq!(
        mapper[0].detail, "LCOM4 2",
        "LCOM4 spans the 6 bodied helpers only, not the 17 declarations"
    );
    assert_eq!(
        god_names(&s),
        ["Service"],
        "the 25-bodied-method class is god; the 23-method, 106-line mapper is not"
    );
    let focus = s.metrics.focus.expect("Java classes are class-like containers");
    assert_eq!(focus.raw, 0.5, "1 god container of 2");
}

/// The NULL rule through the real store read (review fix): an upgraded store
/// whose `has_body` is still `NULL` (migration 25 adds the column empty until
/// re-extraction) reads every declaration as **bodied** — never bodyless — so
/// `function_metrics()` and the snapshot score the mapper exactly as v6 did:
/// LCOM4 19 (17 isolated declarations + the 2 helper components) and a god
/// container by its 23 methods ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
#[test]
fn a_null_has_body_in_the_store_reads_as_bodied() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/main/java/mail/MailboxMapper.java",
        &mapstruct_mapper(),
    );
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    let conn = rusqlite::Connection::open(tmp.path().join(".logos/logos.db"))
        .expect("open the store directly");
    conn.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
    let nulled = conn
        .execute("UPDATE nodes SET has_body = NULL WHERE has_body IS NOT NULL", [])
        .expect("simulate a not-yet-re-extracted store");
    assert_eq!(nulled, 23, "every mapper method had a recorded fact");
    drop(conn);

    let rt = engine.runtime().expect("runtime");
    let facts = rt
        .submit_read(|store| store.function_metrics())
        .expect("function rows read");
    assert!(
        facts.iter().all(|f| f.has_body.is_none()),
        "a NULL column decodes to None, never Some(false)"
    );
    let view = engine
        .hydrate(Granularity::ExcludeContains)
        .expect("dependency view hydrates");
    let s = metrics::snapshot(rt, &view, None, metrics::Thresholds::default())
        .expect("snapshot runs");
    let mapper: Vec<_> = s
        .worst_offenders
        .cohesion
        .iter()
        .filter(|o| o.name == "MailboxMapper")
        .map(|o| o.detail.as_str())
        .collect();
    assert_eq!(mapper, ["LCOM4 19"], "NULL declarations stay in the method set");
    assert_eq!(god_names(&s), ["MailboxMapper"], "23 not-yet-extracted methods → god");
}

/// The template-method shape on real Java extraction (review fix): `run` and
/// `other` both call the `abstract` hook `step()`. The declaration is not
/// counted but still links its callers, so `Base` scores LCOM4 1 — as v6 did —
/// and is no Cohesion offender.
#[test]
fn java_template_method_hook_links_its_callers() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/main/java/app/Base.java",
        "\
package app;

public abstract class Base {
    protected abstract int step();

    public int run() {
        return step() + 1;
    }

    public int other() {
        return step() * 2;
    }
}
",
    );
    let s = scan(tmp.path());
    assert!(
        s.worst_offenders.cohesion.iter().all(|o| o.name != "Base"),
        "the hook links run and other: {:?}",
        s.worst_offenders.cohesion
    );
    assert_eq!(s.metrics.cohesion.expect("Base is scoreable").raw, 1.0);
}

/// [FR-QM-12]'s 25-method AC on Rust: a struct whose impl carries 25 bodied
/// methods is god and a 5-method struct is not. Rust declares `block` as its
/// body kind (S-606), which every `impl` `function_item` carries, so each is
/// bodied ([FR-EX-11]) and the v7 narrowing moves nothing.
/// Ignored: the `impl` methods are not `Contains`-ed by the struct (module
/// docs).
///
/// [FR-QM-12]: ../../docs/specs/requirements/FR-QM-12.md
/// [FR-EX-11]: ../../docs/specs/requirements/FR-EX-11.md
#[test]
#[ignore = "pre-existing extraction gap (S-502 finding): Rust impl / Go receiver methods are Contains-ed by the module, not the struct, so a Rust/Go container has no method count"]
fn rust_struct_with_25_bodied_methods_is_god() {
    let mut src = String::new();
    for (name, n) in [("Big", 25), ("Small", 5)] {
        src.push_str(&format!("pub struct {name} {{\n    x: i32,\n}}\n\nimpl {name} {{\n"));
        for i in 0..n {
            src.push_str(&format!(
                "    pub fn work{i}(&self) -> i32 {{\n        self.x + {i}\n    }}\n"
            ));
        }
        src.push_str("}\n\n");
    }
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib.rs", &src);
    let s = scan(tmp.path());

    assert_eq!(god_names(&s), ["Big"], "25 bodied methods → god; 5 → not");
    assert_eq!(s.metrics.focus.expect("structs are containers").raw, 0.5);
}

/// [FR-QM-12]'s 25-method AC on Go: a type whose method set has 25 bodied
/// methods is god and a 5-method type is not (Go declares no body kind).
/// Ignored: the receiver methods are not `Contains`-ed by the type (module
/// docs).
///
/// [FR-QM-12]: ../../docs/specs/requirements/FR-QM-12.md
#[test]
#[ignore = "pre-existing extraction gap (S-502 finding): Rust impl / Go receiver methods are Contains-ed by the module, not the struct, so a Rust/Go container has no method count"]
fn go_type_with_25_bodied_methods_is_god() {
    let mut src = String::from("package svc\n\n");
    for (name, n) in [("Big", 25), ("Small", 5)] {
        src.push_str(&format!("type {name} struct {{\n\tx int\n}}\n\n"));
        for i in 0..n {
            src.push_str(&format!(
                "func (s *{name}) Work{i}() int {{\n\treturn s.x + {i}\n}}\n\n"
            ));
        }
    }
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "svc/svc.go", &src);
    let s = scan(tmp.path());

    assert_eq!(god_names(&s), ["Big"], "25 bodied methods → god; 5 → not");
    assert_eq!(s.metrics.focus.expect("Go types are containers").raw, 0.5);
}

/// The S-502 decision for header/source-split C++ (coordinator note 3): every
/// member of `Widget` is declared in the header and defined out of line in the
/// `.cpp`, so extraction records 25 bodyless in-class prototypes and captures
/// no out-of-line definition. Bodied LCOM4/Focus **score what remains** —
/// nothing: `Widget` is unscoreable for Cohesion (n/a here, it is the only
/// class) and, at 29 lines, not god by count. A class keeping its bodies
/// in-class is still scored over them (`Inline`: LCOM4 1 over its 2 bodied
/// methods, its 3 prototypes left out).
#[test]
fn cpp_out_of_line_members_score_what_remains() {
    let mut header = String::from("class Widget {\n    int a_;\npublic:\n");
    for i in 0..25 {
        header.push_str(&format!("    void m{i}();\n"));
    }
    header.push_str("};\n");
    let mut source = String::from("#include \"widget.h\"\n\n");
    for i in 0..25 {
        source.push_str(&format!("void Widget::m{i}() {{\n    a_ = {i};\n}}\n\n"));
    }
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/widget.h", &header);
    write(tmp.path(), "src/widget.cpp", &source);
    let split = scan(tmp.path());
    assert!(
        split.metrics.cohesion.is_none(),
        "no bodied method in any class → Cohesion n/a, never a fabricated score"
    );
    assert!(
        god_names(&split).is_empty(),
        "25 prototypes do not make a god container; Widget spans 29 lines"
    );
    assert_eq!(
        split.metrics.focus.expect("Widget is still a container").raw,
        0.0,
        "Focus still counts the split class, as a non-god container"
    );

    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/inline.h",
        "\
class Inline {
    int a_;
public:
    int get() { return this->a_; }
    void set(int v) { this->a_ = v; }
    void p0();
    void p1();
    void p2();
};
",
    );
    let inline = scan(tmp.path());
    let cohesion = inline
        .metrics
        .cohesion
        .expect("Inline has bodied methods → scoreable");
    assert_eq!(
        cohesion.raw, 1.0,
        "the two bodied methods share a_ (LCOM4 1); the 3 prototypes are not components"
    );
}
