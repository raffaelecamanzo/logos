//! C declarations are faithful ([FR-EX-31], [S-582], [CR-185]): a header's
//! `#define`s are Macros, a `struct`/`union`/`enum` is a node only with its
//! body, pointer, array and function-pointer members are Fields, and no
//! function declarator outside a class body is a Method.
//!
//! The headers are those the 2026-10-03 inspection read: redis's `server.h`
//! (`struct redisCommand` mentioned everywhere, defined once, its
//! `redisCommandProc *proc` member) and libuv's `uv.h`, which the grammar
//! cannot read whole — its `extern "C"` guard and its macro-only struct bodies
//! (`struct uv_handle_s { UV_HANDLE_FIELDS };`) tear the file into one ERROR
//! region, every `UV_EXTERN` prototype after them a `field_declaration` of no
//! class.
//!
//! [FR-EX-31]: ../../docs/specs/requirements/FR-EX-31.md
//! [S-582]: ../../docs/planning/journal.md#s-582-c-repositories-own-their-headers-and-c-declarations-are-faithful
//! [CR-185]: ../../docs/requests/CR-185-c-resolves-includes-prototypes-headers-and-macros.md

#![cfg(all(feature = "lang-c", feature = "lang-cpp"))]

use logos_core::extract::{extract, Facts, FileInput, SymbolContext};
use logos_core::model::NodeKind;
use logos_core::plugin::LanguageRegistry;

fn registry() -> LanguageRegistry {
    let tmp = tempfile::tempdir().expect("tempdir");
    LanguageRegistry::load(tmp.path()).expect("embedded grammars load")
}

/// Extract `src` at `path` with the plugin named `language` — a header read as
/// C or as C++, whichever the repository would give it to.
fn facts(language: &str, path: &str, src: &str) -> Facts {
    let reg = registry();
    let plugin = reg
        .iter()
        .find(|p| p.name() == language)
        .unwrap_or_else(|| panic!("the {language} plugin is compiled in"));
    extract(&FileInput::new(path, src), plugin, &SymbolContext::default())
}

/// The `(kind, symbol)` of every node named `name`.
fn named(facts: &Facts, name: &str) -> Vec<(NodeKind, String)> {
    facts
        .nodes
        .iter()
        .filter(|n| n.name == name)
        .map(|n| (n.kind, n.symbol.as_str().to_string()))
        .collect()
}

fn count(facts: &Facts, kind: NodeKind) -> usize {
    facts.nodes.iter().filter(|n| n.kind == kind).count()
}

/// The redis shape: a struct mentioned through pointers, by a forward
/// declaration and in prototypes, and defined once with every member shape.
const SERVER_H: &str = "\
#define CMD_WRITE (1ULL<<0)
#define C_OK 0
struct redisCommand;
struct client;
typedef void redisCommandProc(struct client *c);
typedef int redisGetKeysProc(struct redisCommand *cmd, int argc);
struct redisCommand {
    const char *declared_name;
    redisCommandProc *proc;
    int arity;
    int min_args, max_args;
    int key_specs_num, *key_specs, history[4];
    char **argv;
    int (*getkeys_proc)(struct redisCommand *cmd, int argc);
    void *(*dup)(void *ptr);
    int (*handlers[2])(int);
    void (**hooks)(void);
    struct redisCommand *subcommands[8];
    unsigned int flags : 4;
    union { int id; long ref; } u;
};
int processCommand(struct client *c, struct redisCommand *cmd);
void call(struct client *c, int flags);
enum cmd_flag;
enum cmd_kind { KIND_READ, KIND_WRITE };
";

#[test]
fn a_header_read_as_c_declares_its_macros() {
    let facts = facts("c", "src/server.h", SERVER_H);
    assert_eq!(named(&facts, "CMD_WRITE"), [(NodeKind::Macro, "logos . . . src/`server.h`/CMD_WRITE!".to_string())]);
    assert_eq!(count(&facts, NodeKind::Macro), 2);
}

#[test]
fn a_struct_is_declared_by_its_body_alone() {
    for language in ["c", "cpp"] {
        let facts = facts(language, "src/server.h", SERVER_H);
        let structs = named(&facts, "redisCommand");
        assert_eq!(structs.len(), 1, "{language}: one Struct, not one per mention: {structs:?}");
        assert_eq!(structs[0].0, NodeKind::Struct, "{language}");
        // `struct client *c` and `struct client;` name it; nothing declares it.
        assert!(named(&facts, "client").is_empty(), "{language}: {:?}", named(&facts, "client"));
        // `enum cmd_flag;` names an enum; `enum cmd_kind { … }` declares one.
        assert!(named(&facts, "cmd_flag").is_empty(), "{language}");
        assert_eq!(named(&facts, "cmd_kind").first().map(|n| n.0), Some(NodeKind::Enum), "{language}");
    }
}

#[test]
fn every_member_shape_is_a_field_of_its_struct() {
    for language in ["c", "cpp"] {
        let facts = facts(language, "src/server.h", SERVER_H);
        for member in [
            "declared_name", // T *p
            "proc",          // a typedef'd function type behind a pointer
            "arity",         // T x
            "min_args",      // the first of two plain declarators
            "max_args",      // …and the second
            "key_specs_num", // the first of three declarators
            "key_specs",     // …a pointer one
            "history",       // …an array one
            "argv",          // T **pp
            "getkeys_proc",  // R (*f)(…)
            "dup",           // R *(*f)(…), a pointer return (redis's dictType)
            "handlers",      // R (*f[N])(…)
            "hooks",         // R (**f)(…)
            "subcommands",   // T *a[N]
            "flags",         // a bit field
            "u",             // a member of an anonymous union type
        ] {
            let found = named(&facts, member);
            assert_eq!(found.len(), 1, "{language}: {member}: {found:?}");
            assert_eq!(found[0].0, NodeKind::Field, "{language}: {member}");
            assert!(
                found[0].1.ends_with(&format!("`server.h`/redisCommand#{member}.")),
                "{language}: {member} is the struct's: {}",
                found[0].1
            );
        }
        // The anonymous union's members sit in the member that holds them.
        assert!(named(&facts, "id")[0].1.ends_with("redisCommand#u.id."), "{language}");
    }
}

#[test]
fn a_prototype_outside_a_class_body_is_never_a_method() {
    for language in ["c", "cpp"] {
        let facts = facts(language, "src/server.h", SERVER_H);
        assert_eq!(count(&facts, NodeKind::Method), 0, "{language}");
    }
}

/// The libuv shape, cut down: the `extern "C"` guard, a struct whose body is
/// a macro the grammar never expands, then `UV_EXTERN` prototypes and a struct
/// whose head recovery tears from its body.
const UV_H: &str = "\
#ifndef UV_H
#define UV_H
#ifdef __cplusplus
extern \"C\" {
#endif

#define UV_EXTERN /* nothing */
#define UV_HANDLE_FIELDS \\
  void* data; \\
  uv_loop_t* loop;

typedef struct uv_loop_s uv_loop_t;
typedef struct uv_handle_s uv_handle_t;

struct uv_handle_s {
  UV_HANDLE_FIELDS
};

UV_EXTERN size_t uv_handle_size(uv_handle_type type);
UV_EXTERN uv_handle_type uv_handle_get_type(const uv_handle_t* handle);
UV_EXTERN const char* uv_handle_type_name(uv_handle_type type);
UV_EXTERN void* uv_handle_get_data(const uv_handle_t* handle);

union uv_any_handle {
  UV_HANDLE_TYPE_MAP(XX)
};

struct uv_loop_s {
  /* User data - use this for whatever. */
  void* data;
  unsigned int active_handles;
  struct uv__queue handle_queue;
  union {
    void* unused;
    unsigned int count;
  } active_reqs;
  void* internal_fields;
#ifdef _WIN32
  void* iocp;
#else
  int backend_fd;
#endif
  unsigned int stop_flag;
  UV_LOOP_PRIVATE_FIELDS
};

UV_EXTERN uv_handle_type uv_loop_kind(const uv_loop_t* loop);
UV_EXTERN unsigned int uv_stray_count;
unsigned int uv_stray_flag;
UV_EXTERN void* uv_loop_get_data(const uv_loop_t*);
UV_EXTERN void uv_loop_set_data(uv_loop_t*, void* data);

#ifdef __cplusplus
}
#endif
#endif /* UV_H */
";

#[test]
fn a_torn_header_has_no_method_of_no_class() {
    for language in ["c", "cpp"] {
        let facts = facts(language, "include/uv.h", UV_H);
        let methods: Vec<&str> = facts.nodes.iter().filter(|n| n.kind == NodeKind::Method).map(|n| n.name.as_str()).collect();
        assert!(methods.is_empty(), "{language}: {methods:?}");
    }
}

#[test]
fn a_struct_head_torn_from_its_body_keeps_its_members() {
    let facts = facts("c", "include/uv.h", UV_H);
    let heads = named(&facts, "uv_loop_s");
    assert_eq!(heads.len(), 1, "one uv_loop_s, the definition: {heads:?}");
    assert_eq!(heads[0], (NodeKind::Struct, "logos . . . include/`uv.h`/uv_loop_s#".to_string()));
    // `iocp` and `backend_fd` sit in the branches of an `#ifdef`, `stop_flag` after it.
    for member in ["data", "active_handles", "handle_queue", "active_reqs", "internal_fields", "iocp", "backend_fd", "stop_flag"] {
        let symbols: Vec<String> = named(&facts, member).into_iter().map(|(_, s)| s).collect();
        assert!(
            symbols.iter().any(|s| s.ends_with(&format!("uv_loop_s#{member}."))),
            "{member} is uv_loop_s's: {symbols:?}"
        );
    }
    assert!(named(&facts, "count")[0].1.ends_with("uv_loop_s#active_reqs.count."));
    // Its body ends at its closing brace: what recovery left after it is no
    // member of it.
    for after in ["uv_handle_type", "uv_stray_count", "uv_stray_flag"] {
        assert!(named(&facts, after).iter().all(|(_, s)| !s.contains("uv_loop_s#")), "{after}: {:?}", named(&facts, after));
    }
}

#[test]
fn a_member_no_declaration_holds_is_none() {
    // `UV_EXTERN uv_handle_type uv_handle_get_type(…)` tears into a
    // `field_declaration` whose name is `uv_handle_type`, outside every body —
    // once after `uv_loop_s`'s closing brace, so its torn body ends there. A
    // header read as C drops it.
    let facts = facts("c", "include/uv.h", UV_H);
    assert!(named(&facts, "uv_handle_type").is_empty(), "{:?}", named(&facts, "uv_handle_type"));
}

#[test]
fn a_cpp_member_of_an_uncaptured_container_stays_a_field() {
    // C++ drops no member: the containers its query does not name — a template
    // specialisation, a nested class defined out of line — still hold fields.
    let facts = facts(
        "cpp",
        "src/box.hpp",
        "\
template <class T> struct Box { T value; };
template <> struct Box<int> { int special; };
struct Widget { struct Impl; Impl* impl_; };
struct Widget::Impl { int counter; };
",
    );
    for member in ["value", "special", "impl_", "counter"] {
        assert_eq!(named(&facts, member).first().map(|n| n.0), Some(NodeKind::Field), "{member}");
    }
}

#[test]
fn a_brace_that_does_not_follow_struct_or_union_is_no_head() {
    // The near miss of the torn head: a name and a brace loose in an ERROR,
    // written after a type name rather than `struct`. Recovery leaves it the
    // same shape; the source says it declares no struct.
    let torn = UV_H.replace("struct uv_loop_s {", "uv_loop_s {");
    let near_miss = facts("c", "include/uv.h", &torn);
    assert!(
        named(&near_miss, "uv_loop_s").iter().all(|(kind, _)| *kind != NodeKind::Struct),
        "{:?}",
        named(&near_miss, "uv_loop_s")
    );
    // Nor is it a declaration the parse error truncated (FR-EX-30): the real
    // head is one, so the near miss reports one fewer.
    let truncated = |facts: &Facts| -> usize {
        let warning = facts.warnings.iter().find(|w| w.contains("truncated")).expect("a parse-damage warning");
        let clause = warning.split("; ").find(|c| c.contains("truncated")).expect("the truncated clause");
        clause.split_whitespace().next().and_then(|n| n.parse().ok()).expect("a count")
    };
    assert_eq!(truncated(&near_miss) + 1, truncated(&facts("c", "include/uv.h", UV_H)));
}

#[test]
fn a_class_body_keeps_its_methods() {
    // The guard's other side: every in-class shape is still a Method — a
    // pointer and a reference return, an inline definition, and a member in
    // each branch of an `#ifdef`, of an `#if`/`#elif`/`#else` chain, and of an
    // `#ifdef` nested in another.
    let facts = facts(
        "cpp",
        "src/widget.h",
        "\
class Widget {
public:
  int* get();
  Widget& self();
  void set(int v) { x_ = v; }
#ifdef FAST
  void fast();
#else
  void slow();
#endif
#if A
  void a();
#elif B
  void b();
#else
  void c();
#endif
#ifdef X
#ifdef Y
  void nested();
#endif
#endif
private:
  int x_;
  int (*hook_)(int);
};
",
    );
    for method in ["get", "self", "set", "fast", "slow", "a", "b", "c", "nested"] {
        assert_eq!(named(&facts, method).first().map(|n| n.0), Some(NodeKind::Method), "{method}");
    }
    assert_eq!(named(&facts, "hook_").first().map(|n| n.0), Some(NodeKind::Field));
    assert_eq!(named(&facts, "x_").first().map(|n| n.0), Some(NodeKind::Field));
}

#[test]
fn a_c_source_symbol_is_unchanged() {
    // FR-EX-31: every pre-existing C symbol that still exists keeps its id.
    let facts = facts("c", "src/server.c", "int processCommand(void) { return 0; }\nstatic int helper(void) { return 1; }\n");
    assert_eq!(named(&facts, "processCommand"), [(NodeKind::Function, "logos . . . src/`server.c`/processCommand().".to_string())]);
}
