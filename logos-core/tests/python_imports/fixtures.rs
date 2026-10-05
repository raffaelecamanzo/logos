//! The fixtures of `python_imports.rs` (S-519): small trees shaped like the
//! inspection estates they stand for — a werkzeug-style `src/` library with
//! relative imports and `__init__` re-exports, a healthchecks-style Django
//! project rooted at the repository — each pairing what must bind with the near
//! miss that must not.

/// `(project-relative path, source)` pairs.
pub type Fixture = &'static [(&'static str, &'static str)];

/// A werkzeug-shaped `src/` package: relative imports one and two levels up, an
/// `__init__.py` that re-exports a submodule's class, a test outside the root
/// importing the installed package by name, and an external import.
pub const WERKZEUG: Fixture = &[
    (
        "src/werkzeug/__init__.py",
        "from .serving import run_simple as run_simple\n\n__version__ = \"3.1\"\n",
    ),
    (
        "src/werkzeug/serving.py",
        "def run_simple(hostname, port, application):\n    return application\n",
    ),
    (
        "src/werkzeug/_internal.py",
        "def _wsgi_decoding_dance(s):\n    return s\n",
    ),
    (
        "src/werkzeug/routing/__init__.py",
        "from .map import Map as Map\nfrom .rules import Rule as Rule\n",
    ),
    (
        "src/werkzeug/routing/rules.py",
        "import re\n\nfrom .._internal import _wsgi_decoding_dance\n\n\nclass Rule:\n    def __init__(self, string):\n        self.rule = _wsgi_decoding_dance(string)\n",
    ),
    (
        "src/werkzeug/routing/map.py",
        "from .rules import Rule\nfrom . import rules\n\n\nclass Map:\n    def add(self, rule):\n        return rule\n",
    ),
    (
        "tests/test_routing.py",
        "import pytest\n\nfrom werkzeug import routing\nfrom werkzeug.routing import Map\nfrom werkzeug.routing.rules import Rule\n\n\ndef test_basic():\n    assert Map and Rule and routing\n",
    ),
];

/// A healthchecks-shaped Django project: packages at the repository root, so
/// the root is the import root; absolute imports across apps; a namespace
/// package (`hc/lib/` has no `__init__.py`) that still descends; and a script
/// in a `src/` that holds no package, which must not become a root.
pub const HEALTHCHECKS: Fixture = &[
    ("hc/__init__.py", ""),
    ("hc/api/__init__.py", ""),
    (
        "hc/api/models.py",
        "from django.db import models\n\n\nclass Check(models.Model):\n    pass\n\n\nclass Flip(models.Model):\n    pass\n",
    ),
    (
        "hc/lib/date.py",
        "def format_duration(td):\n    return str(td)\n",
    ),
    (
        "hc/front/views.py",
        "from hc.api.models import Check, Flip\nfrom hc.lib.date import format_duration\n\n\ndef details(request):\n    return format_duration(Check)\n",
    ),
    ("src/tool.py", "def main():\n    pass\n"),
];

/// A JavaScript entry module and a Rust binary: `main` is a package-file stem
/// only where the Rust plugin declares it.
pub const MAIN_JS: Fixture = &[
    ("web/src/main.js", "export function boot() { return 1; }\n"),
    ("cli/src/main.rs", "fn main() {}\n"),
    ("cli/src/cmd/mod.rs", "pub fn run() {}\n"),
];

/// A Java test extending a Kotlin base class across the one `jvm` family — the
/// koin `UnitJavaTest extends KoinCoreTest` shape — beside a C# `using` of a
/// namespace a PHP file also declares, which another family can never name.
pub const FAMILIES: Fixture = &[
    (
        "core/src/jvmTest/kotlin/org/koin/test/KoinCoreTest.kt",
        "package org.koin.test\n\nabstract class KoinCoreTest\n",
    ),
    (
        "core/src/jvmTest/java/org/koin/java/UnitJavaTest.java",
        "package org.koin.java;\n\nimport org.koin.test.KoinCoreTest;\n\npublic class UnitJavaTest extends KoinCoreTest {\n}\n",
    ),
    (
        "app/Models/User.php",
        "<?php\n\nnamespace App\\Models;\n\nclass User\n{\n}\n",
    ),
    (
        "src/Api/OrdersApi.cs",
        "using App.Models;\n\nnamespace App.Api;\n\npublic class OrdersApi\n{\n}\n",
    ),
];

/// werkzeug's `wrappers/request.py` shape (S-519 T2): every method re-runs the
/// same imports — the external `import warnings`, an `import werkzeug` whose
/// head expands to itself, and an aliased `from` import of an in-repository
/// class it then calls by the alias. The methods also import a `_Fallback`, two
/// of them from `compat` and two from `legacy`: a genuine rival, however often
/// each side is repeated. Four methods; the real file repeats `import
/// warnings` 13 times.
pub const REPEATED_IMPORTS: Fixture = &[
    ("src/werkzeug/__init__.py", ""),
    (
        "src/werkzeug/exceptions.py",
        "class BadRequest(Exception):\n    pass\n",
    ),
    (
        "src/werkzeug/compat.py",
        "class BadRequest(Exception):\n    pass\n",
    ),
    (
        "src/werkzeug/legacy.py",
        "class BadRequest(Exception):\n    pass\n",
    ),
    ("src/werkzeug/wrappers/__init__.py", ""),
    (
        "src/werkzeug/wrappers/request.py",
        "class Request:\n    def on_json_loading_failed(self):\n        import warnings\n        import werkzeug\n        from werkzeug.exceptions import BadRequest as _BadRequest\n        from werkzeug.compat import BadRequest as _Fallback\n\n        warnings.warn(\"deprecated\", DeprecationWarning)\n        return _BadRequest(werkzeug), _Fallback(werkzeug)\n\n    def get_json(self):\n        import warnings\n        import werkzeug\n        from werkzeug.exceptions import BadRequest as _BadRequest\n        from werkzeug.compat import BadRequest as _Fallback\n\n        warnings.warn(\"deprecated\", DeprecationWarning)\n        return _BadRequest(werkzeug), _Fallback(werkzeug)\n\n    def close(self):\n        import warnings\n        import werkzeug\n        from werkzeug.exceptions import BadRequest as _BadRequest\n        from werkzeug.legacy import BadRequest as _Fallback\n\n        warnings.warn(\"deprecated\", DeprecationWarning)\n        return _BadRequest(werkzeug), _Fallback(werkzeug)\n\n    def stream(self):\n        import warnings\n        import werkzeug\n        from werkzeug.exceptions import BadRequest as _BadRequest\n        from werkzeug.legacy import BadRequest as _Fallback\n\n        warnings.warn(\"deprecated\", DeprecationWarning)\n        return _BadRequest(werkzeug), _Fallback(werkzeug)\n",
    ),
];
