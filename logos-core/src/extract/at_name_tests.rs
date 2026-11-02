//! Unit tests for the one settle pass of fields declared at their name (S-837,
//! [CR-264]).
//!
//! The pass is driven over hand-built declarations: each fixture is parsed for
//! its nodes only, and the declarations — their kinds, names, parents and rules
//! — are written by hand, as `assign_parents` would leave them. No query and no
//! language plugin decides a rule, so each rule is pinned on its own: both
//! owners, both precedences, and the three drops (no owner, a `this`-rebinding
//! function, a name already declared as a Field).
//!
//! [CR-264]: ../../../docs/requests/CR-264-sprint-109-review-follow-ups.md

use std::collections::{BTreeMap, BTreeSet};

use tree_sitter::{CaptureQuantifier, Node, Parser, Query, Tree};

use super::{name, settle, Order, Owner, Rules};
use crate::extract::{assoc, definition, member, Decl};
use crate::model::NodeKind;
use crate::plugin::LanguageRegistry;

/// The loaded registry, built once per test binary.
fn registry() -> &'static LanguageRegistry {
    static ONCE: std::sync::OnceLock<LanguageRegistry> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let tmp = tempfile::tempdir().expect("tempdir");
        LanguageRegistry::load(tmp.path()).expect("registry loads")
    })
}

/// The parse of `source` under the grammar `path`'s extension names.
fn parse(path: &str, source: &str) -> Tree {
    let plugin = registry().for_path(path).expect("a grammar for the fixture");
    let mut parser = Parser::new();
    parser.set_language(plugin.language()).expect("grammar binds");
    let tree = parser.parse(source, None).expect("parses");
    assert!(!tree.root_node().has_error(), "{path} parses cleanly");
    tree
}

/// The `nth` node (from 0, in source order) of `kind` whose text is `text`.
fn node<'t>(tree: &'t Tree, source: &str, kind: &str, text: &str, nth: usize) -> Node<'t> {
    let mut found = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(n) = stack.pop() {
        if n.kind() == kind && n.utf8_text(source.as_bytes()) == Ok(text) {
            found.push(n);
        }
        let mut cursor = n.walk();
        stack.extend(n.children(&mut cursor));
    }
    found.sort_by_key(|n| n.start_byte());
    *found.get(nth).unwrap_or_else(|| panic!("no {kind} `{text}` #{nth}"))
}

/// The declaration of `node` named `name`, as `assign_parents` leaves it.
fn decl<'t>(node: Node<'t>, kind: NodeKind, name: &str, parent: Option<usize>, at_name: Option<Rules>) -> Decl<'t> {
    Decl {
        node,
        kind,
        name: name.to_string(),
        start_byte: node.start_byte(),
        start_line: node.start_position().row as u32 + 1,
        end_line: node.end_position().row as u32 + 1,
        parent,
        ordinal: 0,
        self_type: None,
        params: None,
        item: assoc::DeclItemFacts::default(),
        def: definition::DeclDefinitionFacts::default(),
        qualifier: Vec::new(),
        completes: None,
        at_name,
        owner: None,
        member: member::DeclMemberFacts::default(),
        signature: None,
    }
}

const SCOPE_BODY_FIRST: Rules = Rules {
    owner: Owner::Scope,
    order: Order::BodyFirst,
};

/// Every Field the pass kept, as (name, line, owner's name).
fn fields(decls: &[Decl<'_>]) -> BTreeSet<(String, u32, String)> {
    decls
        .iter()
        .filter(|d| d.kind == NodeKind::Field)
        .map(|d| {
            let owner = match d.parent {
                None => "<none>".to_string(),
                Some(p) => decls.get(p).map_or("<dangling>".to_string(), |o| o.name.clone()),
            };
            (d.name.clone(), d.start_line, owner)
        })
        .collect()
}

fn set(rows: &[(&str, u32, &str)]) -> BTreeSet<(String, u32, String)> {
    rows.iter().map(|&(n, l, o)| (n.to_string(), l, o.to_string())).collect()
}

const PY: &str = "\
def g():
    pass
class A:
    x = 1
    def m(self):
        self.x = 2
        self.y = 3
        def inner():
            self.q = 4
    y = 5
def free(self):
    self.f = 6
class B:
    def k(self):
        self.x = 7
";

/// The Python fixture's declarations, every field under `rules`. No owner is
/// the first declaration, and kept declarations follow dropped ones, so an
/// owner compared by index or a parent left unremapped shows.
fn python_decls<'t>(tree: &'t Tree, rules: Rules) -> Vec<Decl<'t>> {
    let def = |name: &str| node(tree, PY, "identifier", name, 0).parent().expect("a definition");
    let field = |name: &str, nth: usize, parent: usize| {
        decl(node(tree, PY, "identifier", name, nth), NodeKind::Field, name, Some(parent), Some(rules))
    };
    vec![
        decl(def("g"), NodeKind::Function, "g", None, None),            // 0
        decl(def("A"), NodeKind::Class, "A", None, None),               // 1
        decl(def("m"), NodeKind::Method, "m", Some(1), None),           // 2
        decl(def("inner"), NodeKind::Function, "inner", Some(2), None), // 3
        decl(def("free"), NodeKind::Function, "free", None, None),      // 4
        field("x", 0, 1),                                               // 5: line 4, class body
        field("x", 1, 2),                                               // 6: line 6, `m`
        field("y", 0, 2),                                               // 7: line 7, `m`
        field("q", 0, 3),                                               // 8: line 9, nested function
        field("y", 1, 1),                                               // 9: line 10, class body
        field("f", 0, 4),                                               // 10: line 12, free function
        decl(def("B"), NodeKind::Class, "B", None, None),               // 11
        // A callable of either kind is a class's method.
        decl(def("k"), NodeKind::Function, "k", Some(11), None),        // 12
        field("x", 2, 12),                                              // 13: line 15, `k`
    ]
}

#[test]
fn the_scope_owner_is_the_class_or_the_methods_class_and_a_class_body_write_comes_first() {
    let tree = parse("a.py", PY);
    let mut decls = python_decls(&tree, SCOPE_BODY_FIRST);
    settle(&mut decls);
    // `x`: the class body's (line 4) over the method's; `y`: the class body's
    // (line 10) over the method's earlier write (line 7); `q` (a function
    // nested in the method) and `f` (a free function) have no class; `B` keeps
    // its own `x`, lifted from `k`.
    assert_eq!(fields(&decls), set(&[("x", 4, "A"), ("y", 10, "A"), ("x", 15, "B")]));
    // The compaction remaps every parent: `inner` is still `m`'s, and `k` —
    // after four dropped declarations — still `B`'s.
    let parent_of = |name: &str| {
        let d = decls.iter().find(|d| d.name == name).expect("kept");
        decls.get(d.parent.expect("a parent")).map(|p| p.name.clone())
    };
    assert_eq!(parent_of("inner").as_deref(), Some("m"));
    assert_eq!(parent_of("k").as_deref(), Some("B"));
}

#[test]
fn source_order_keeps_the_first_write_wherever_it_is() {
    let tree = parse("a.py", PY);
    let rules = Rules {
        owner: Owner::Scope,
        order: Order::Source,
    };
    let mut decls = python_decls(&tree, rules);
    settle(&mut decls);
    // `y`: the method's write (line 7) is first in source order, and is lifted
    // from `m` to its class.
    assert_eq!(fields(&decls), set(&[("x", 4, "A"), ("y", 7, "A"), ("x", 15, "B")]));
}

#[test]
fn the_enclosing_owner_is_the_nearest_class_or_module_through_any_method_or_function() {
    let tree = parse("a.py", PY);
    let rules = Rules {
        owner: Owner::Enclosing,
        order: Order::Source,
    };
    let mut decls = python_decls(&tree, rules);
    settle(&mut decls);
    // `q`, in a function nested in the method, is `A`'s; `f` has no class.
    assert_eq!(fields(&decls), set(&[("x", 4, "A"), ("y", 7, "A"), ("q", 9, "A"), ("x", 15, "B")]));
}

const RB: &str = "\
module M
  def self.included(base)
    [1].each { @a = 1 }
  end
  class K
    def m
      @b = 1
    end
  end
end
@top = 1
";

#[test]
fn the_enclosing_owner_is_the_nearest_module_or_class_through_a_block_and_drops_what_none_holds() {
    let tree = parse("a.rb", RB);
    let rules = Rules {
        owner: Owner::Enclosing,
        order: Order::Source,
    };
    let module = node(&tree, RB, "constant", "M", 0).parent().expect("the module");
    let method = node(&tree, RB, "identifier", "included", 0).parent().expect("the method");
    let class = node(&tree, RB, "constant", "K", 0).parent().expect("the class");
    let class_method = node(&tree, RB, "identifier", "m", 0).parent().expect("the class's method");
    let mut decls = vec![
        decl(module, NodeKind::Module, "M", None, None),
        decl(method, NodeKind::Method, "included", Some(0), None),
        decl(class, NodeKind::Class, "K", Some(0), None),
        decl(class_method, NodeKind::Method, "m", Some(2), None),
        decl(node(&tree, RB, "instance_variable", "@a", 0), NodeKind::Field, "@a", Some(1), Some(rules)),
        decl(node(&tree, RB, "instance_variable", "@b", 0), NodeKind::Field, "@b", Some(3), Some(rules)),
        decl(node(&tree, RB, "instance_variable", "@top", 0), NodeKind::Field, "@top", None, Some(rules)),
    ];
    settle(&mut decls);
    // `@b` is the nearest owner's, `K`, not the outer module's.
    assert_eq!(fields(&decls), set(&[("@a", 3, "M"), ("@b", 7, "K")]));
}

#[test]
fn body_first_applies_under_the_enclosing_owner_too() {
    // An `attr_reader :x` after the `@x = …` of a method: source order keeps the
    // method's, body-first the class body's.
    let src = "class K\n  def m\n    @x = 1\n  end\n  attr_reader :x\nend\n";
    let tree = parse("a.rb", src);
    let class = node(&tree, src, "constant", "K", 0).parent().expect("the class");
    let method = node(&tree, src, "identifier", "m", 0).parent().expect("the method");
    let build = |order| {
        let rules = Rules {
            owner: Owner::Enclosing,
            order,
        };
        vec![
            decl(class, NodeKind::Class, "K", None, None),
            decl(method, NodeKind::Method, "m", Some(0), None),
            decl(node(&tree, src, "instance_variable", "@x", 0), NodeKind::Field, "@x", Some(1), Some(rules)),
            decl(node(&tree, src, "simple_symbol", ":x", 0), NodeKind::Field, "@x", Some(0), Some(rules)),
        ]
    };
    let mut source = build(Order::Source);
    settle(&mut source);
    assert_eq!(fields(&source), set(&[("@x", 3, "K")]));
    let mut body_first = build(Order::BodyFirst);
    settle(&mut body_first);
    assert_eq!(fields(&body_first), set(&[("@x", 5, "K")]));
}

const JS: &str = "\
class C {
  x = 0;
  constructor() {
    this.x = 1;
    this.y = 2;
    this.z = 3;
  }
  z() {}
  m() {
    setTimeout(function () { this.t = 1; });
    setTimeout(function* () { this.g = 1; });
    setTimeout(() => { this.u = 1; });
    function* gen() { this.gd = 1; }
  }
  handler = () => { this.h = 1; };
}
";

#[test]
fn a_this_rebinding_function_and_a_name_the_class_declares_drop_the_write() {
    let tree = parse("a.js", JS);
    let class = node(&tree, JS, "class_declaration", JS.trim_end(), 0);
    let prop = |name: &str, nth: usize| node(&tree, JS, "property_identifier", name, nth);
    let method = |name: &str, nth: usize| prop(name, nth).parent().expect("a method");
    let rules = Some(SCOPE_BODY_FIRST);
    let mut decls = vec![
        decl(class, NodeKind::Class, "C", None, None),                                 // 0
        decl(prop("x", 0), NodeKind::Field, "x", Some(0), None),                       // 1: declared in its own right
        decl(method("constructor", 0), NodeKind::Method, "constructor", Some(0), None), // 2
        decl(prop("x", 1), NodeKind::Field, "x", Some(2), rules),                      // 3
        decl(prop("y", 0), NodeKind::Field, "y", Some(2), rules),                      // 4
        decl(prop("z", 0), NodeKind::Field, "z", Some(2), rules),                      // 5
        decl(method("z", 1), NodeKind::Method, "z", Some(0), None),                    // 6: a method, not a Field
        decl(method("m", 0), NodeKind::Method, "m", Some(0), None),                    // 7
        decl(prop("t", 0), NodeKind::Field, "t", Some(7), rules),                      // 8: in a `function`
        decl(prop("g", 0), NodeKind::Field, "g", Some(7), rules),                      // 9: in a `function*`
        decl(prop("u", 0), NodeKind::Field, "u", Some(7), rules),                      // 10: in an arrow
        // Under `m`, uncaptured `gen` aside: only its `function*` drops it.
        decl(prop("gd", 0), NodeKind::Field, "gd", Some(7), rules),                    // 11
        decl(prop("handler", 0).parent().expect("a field"), NodeKind::Field, "handler", Some(0), None), // 12
        // Its scope is the class's, but a Field, no method: no class owns it.
        decl(prop("h", 0), NodeKind::Field, "h", Some(12), rules),                     // 13
    ];
    settle(&mut decls);
    // `x`: the class's own Field is the declaration; `z`: a method of the name
    // declares no Field, so the write does; `t`, `g` and `gd`: their
    // `function`, `function*` expression and `function*` declaration rebind
    // `this`; `h`: written in a Field of the class, not a method of it.
    assert_eq!(
        fields(&decls),
        set(&[("x", 2, "C"), ("y", 5, "C"), ("z", 6, "C"), ("u", 12, "C"), ("handler", 15, "C")])
    );
    assert!(
        decls.iter().filter(|d| d.name == "x").all(|d| d.at_name.is_none()),
        "the class's own `x` is the one kept"
    );
}

/// A one-pattern query over the Ruby grammar with `settings`.
fn query(settings: &str) -> Query {
    let plugin = registry().for_path("a.rb").expect("ruby");
    Query::new(plugin.language(), &format!("((identifier) @symbol.field @symbol.at_name {settings})")).expect("compiles")
}

#[test]
fn a_sigil_names_the_field_whatever_the_capture_writes() {
    let q = query(r#"(#set! symbol.field_sigil "@")"#);
    for captured in ["@x", ":x", "x"] {
        assert_eq!(name(&q, 0, captured), "@x", "{captured}");
    }
    let plain = query("");
    assert_eq!(name(&plain, 0, ":x"), ":x");
}

#[test]
fn the_settings_name_the_rules_and_their_absence_is_the_default() {
    let q = query(r#"(#set! symbol.field_owner "enclosing") (#set! symbol.field_order "source")"#);
    assert_eq!(
        Rules::of_pattern(&q, 0),
        Rules {
            owner: Owner::Enclosing,
            order: Order::Source
        }
    );
    let q = query(r#"(#set! symbol.field_owner "scope") (#set! symbol.field_order "body-first")"#);
    assert_eq!(Rules::of_pattern(&q, 0), SCOPE_BODY_FIRST);
    assert_eq!(Rules::of_pattern(&query(""), 0), SCOPE_BODY_FIRST);
}

/// The embedded `symbols` queries spell the vocabulary right, so no rule falls
/// back to a default by a typo: every `symbol.field_*` key is one the
/// vocabulary knows and names a value it knows, and every pattern carrying the
/// marker states its owner and order and marks every field it captures — each
/// branch of an alternation included. Python, JavaScript and Ruby declare their
/// fields with the one marker, on the patterns each declares them by.
#[test]
fn every_embedded_query_names_known_rules_with_the_one_marker() {
    const KNOWN: &[(&str, &[&str])] = &[
        ("symbol.field_owner", &["scope", "enclosing"]),
        ("symbol.field_order", &["body-first", "source"]),
        ("symbol.field_sigil", &["@"]),
    ];
    let mut marked: BTreeMap<String, usize> = BTreeMap::new();
    for plugin in registry().iter() {
        let Some(q) = plugin.query("symbols") else { continue };
        let marker = q.capture_index_for_name(super::CAPTURE);
        let field = q.capture_index_for_name("symbol.field");
        for pattern in 0..q.pattern_count() {
            let at = |pattern: usize| format!("{}: pattern {pattern}", plugin.name());
            for setting in q.property_settings(pattern).iter().filter(|p| p.key.starts_with("symbol.field_")) {
                let known = KNOWN.iter().find(|(key, _)| **key == *setting.key);
                let (_, values) = known.unwrap_or_else(|| panic!("{} sets an unknown {}", at(pattern), setting.key));
                let value = setting.value.as_deref().unwrap_or_default();
                assert!(values.contains(&value), "{} sets {} {value:?}", at(pattern), setting.key);
            }
            let quantifier = |index: Option<u32>| index.map_or(CaptureQuantifier::Zero, |i| q.capture_quantifiers(pattern)[i as usize]);
            if quantifier(marker) != CaptureQuantifier::Zero {
                assert_eq!(quantifier(marker), quantifier(field), "{} marks only some of its fields", at(pattern));
                *marked.entry(plugin.name().to_string()).or_default() += 1;
                for key in ["symbol.field_owner", "symbol.field_order"] {
                    let set = crate::plugin::queries::pattern_setting(q, pattern, key);
                    assert!(set.is_some(), "{} carries the marker without {key}", at(pattern));
                }
            }
        }
    }
    let want: BTreeMap<String, usize> =
        [("javascript", 1), ("python", 2), ("ruby", 4)].into_iter().map(|(l, n)| (l.to_string(), n)).collect();
    assert_eq!(marked, want, "the patterns carrying the marker, per plugin");
}
