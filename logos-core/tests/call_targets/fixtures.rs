//! Fixtures for `tests/call_targets.rs` (S-521): the shapes the 2026-10-03
//! language inspection measured — healthchecks' `Check(project=p)`, a Kotlin
//! `Foo()`, and libuv's `src/fs-poll.c` macro — each beside the negatives that
//! must stay unbound.

pub type Fixture = &'static [(&'static str, &'static str)];

/// healthchecks' layout (its repository root is the import root): a view
/// constructs a model class through a `from` import, and calls a function
/// through the same import. A class declared twice in one module and a library
/// class stay unbound.
pub const PYTHON: Fixture = &[
    ("hc/__init__.py", ""),
    ("hc/api/__init__.py", ""),
    (
        "hc/api/models.py",
        "class Project:\n    pass\n\n\nclass Check:\n    def __init__(self, project):\n        self.project = project\n\n\ndef prepare(check):\n    return check\n",
    ),
    (
        "hc/front/views.py",
        "from django.http import HttpResponse\n\nfrom hc.api.models import Check, Project, prepare\n\n\ndef add_check(request):\n    p = Project()\n    check = Check(project=p)\n    prepare(check)\n    return HttpResponse()\n",
    ),
    (
        "hc/front/compat.py",
        "import sys\n\nif sys.version_info >= (3, 11):\n    class Clock:\n        pass\nelse:\n    class Clock:\n        pass\n\n\ndef now():\n    return Clock()\n\n\ndef later():\n    return Missing()\n",
    ),
];

/// A Kotlin class constructed through a single-type import and from its own
/// package; a class two source sets declare under one package, a name no
/// declaration carries, a bare `Foo()` inside a class body, and a class beside
/// a factory function of its name (`fun Job(s: String): Job`) stay unbound.
pub const KOTLIN: Fixture = &[
    ("src/main/kotlin/com/x/Foo.kt", "package com.x\n\nclass Foo\n"),
    ("src/main/kotlin/com/x/Make.kt", "package com.x\n\nfun make() = Foo()\n"),
    (
        "src/main/kotlin/com/y/Use.kt",
        "package com.y\n\nimport com.x.Foo\n\nfun build() = Foo()\n\nfun missing() = Missing()\n\nclass Svc {\n    fun go() = Foo()\n}\n",
    ),
    ("src/main/kotlin/com/z/Bar.kt", "package com.z\n\nclass Bar\n"),
    ("src/test/kotlin/com/z/Bar.kt", "package com.z\n\nclass Bar\n"),
    ("src/main/kotlin/com/z/Mk.kt", "package com.z\n\nfun mk() = Bar()\n"),
    (
        "src/main/kotlin/com/f/Job.kt",
        "package com.f\n\nclass Job(val n: Int)\n\nfun Job(s: String): Job = Job(s.length)\n",
    ),
    ("src/main/kotlin/com/f/Start.kt", "package com.f\n\nfun start() = Job(\"a\")\n"),
    (
        "src/main/kotlin/com/g/Run.kt",
        "package com.g\n\nimport com.f.Job\n\nfun run() = Job(\"b\")\n",
    ),
];

/// libuv's `src/fs-poll.c`: it defines `uv__make_close_pending` as a macro and
/// calls it, while `src/unix/core.c` defines and calls a function of the same
/// name. A file defining a macro and a function of one name leaves a call to it
/// unbound.
pub const C: Fixture = &[
    (
        "src/fs-poll.c",
        "#define uv__make_close_pending(h) uv__want_endgame((h)->loop, (h))\n\nstatic void poll_cb(void* handle) {\n  uv__make_close_pending(handle);\n}\n",
    ),
    (
        "src/unix/core.c",
        "void uv__make_close_pending(void* handle) {\n}\n\nvoid uv__close(void* handle) {\n  uv__make_close_pending(handle);\n}\n",
    ),
    (
        "src/dual.c",
        "#define dual(x) dual_impl(x)\n\nint dual(int x) {\n  return x;\n}\n\nint use_dual(void) {\n  return dual(1);\n}\n",
    ),
];

/// Rust declares neither key: a call to a `macro_rules!` name or to a struct
/// stays a callable-only lookup, and a call to a function binds as before.
pub const RUST: Fixture = &[(
    "tool/src/lib.rs",
    "macro_rules! twice {\n    ($e:expr) => { $e + $e };\n}\n\npub struct Point;\n\npub fn run() -> i32 {\n    helper();\n    Point();\n    twice(1)\n}\n\nfn helper() {}\n",
)];
