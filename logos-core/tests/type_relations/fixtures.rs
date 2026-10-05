//! Fixtures for `tests/type_relations.rs` (S-522): the inheritance shapes the
//! 2026-10-03 language inspection measured — werkzeug's aliased
//! `Request(_SansIORequest)`, healthchecks' `BaseTestCase`, monolog's handlers
//! and traits, Newtonsoft's readers, and Kotlin's mixed supertype lists — each
//! beside the negatives that must stay unbound.

pub type Fixture = &'static [(&'static str, &'static str)];

/// werkzeug's layout (`src/` is its import root): the WSGI `Request` extends
/// the sans-IO one through a relative import that renames it, and calls up to
/// it through `super()` and `self`. Converters share one base. A library base
/// (`Generic[T]`), a metaclass keyword and a class the module never imports
/// stay unbound.
pub const WERKZEUG: Fixture = &[
    ("src/werkzeug/__init__.py", ""),
    ("src/werkzeug/sansio/__init__.py", ""),
    (
        "src/werkzeug/sansio/request.py",
        "class Request:\n    def close(self):\n        pass\n\n    def get_data(self):\n        return b\"\"\n",
    ),
    ("src/werkzeug/wrappers/__init__.py", ""),
    (
        "src/werkzeug/wrappers/request.py",
        "from ..sansio.request import Request as _SansIORequest\n\n\nclass Request(_SansIORequest):\n    def close(self):\n        super().close()\n\n    def data(self):\n        return self.get_data()\n",
    ),
    ("src/werkzeug/routing/__init__.py", ""),
    (
        "src/werkzeug/routing/converters.py",
        "import typing as t\n\n\nclass BaseConverter:\n    regex = \"[^/]+\"\n\n\nclass UnicodeConverter(BaseConverter):\n    pass\n\n\nclass UUIDConverter(BaseConverter):\n    pass\n\n\nclass Box(t.Generic[t.T], metaclass=Meta):\n    pass\n\n\nclass IntBox(Box[int]):\n    pass\n\n\nclass Orphan(Unimported):\n    pass\n",
    ),
    (
        "src/werkzeug/datastructures.py",
        "class Unimported:\n    pass\n",
    ),
];

/// healthchecks' layout (its repository root is the import root): every test
/// module subclasses `hc.test.BaseTestCase`, which subclasses Django's
/// `TestCase` — external, so it stays unbound.
pub const HEALTHCHECKS: Fixture = &[
    ("hc/__init__.py", ""),
    (
        "hc/test.py",
        "from django.test import TestCase\n\n\nclass BaseTestCase(TestCase):\n    def setUp(self):\n        pass\n",
    ),
    ("hc/api/__init__.py", ""),
    ("hc/api/tests/__init__.py", ""),
    (
        "hc/api/tests/test_ping.py",
        "from hc.test import BaseTestCase\n\n\nclass PingTestCase(BaseTestCase):\n    def setUp(self):\n        super().setUp()\n",
    ),
    (
        "hc/api/tests/test_badge.py",
        "from hc import test\n\n\nclass BadgeTestCase(test.BaseTestCase):\n    pass\n",
    ),
];

/// monolog's shape: an interface, an abstract handler that implements it and
/// `use`s a trait from another namespace, concrete handlers that extend the
/// abstract one or implement the interface through an aliased `use`, an
/// interface that extends the interface, and a class named like the global
/// `\Exception` it extends.
pub const MONOLOG: Fixture = &[
    (
        "src/Monolog/Handler/HandlerInterface.php",
        "<?php\n\nnamespace Monolog\\Handler;\n\ninterface HandlerInterface\n{\n    public function handle(array $record): bool;\n}\n",
    ),
    (
        "src/Monolog/Handler/FormattableHandlerInterface.php",
        "<?php\n\nnamespace Monolog\\Handler;\n\ninterface FormattableHandlerInterface extends HandlerInterface\n{\n}\n",
    ),
    (
        "src/Monolog/Handler/AbstractHandler.php",
        "<?php\n\nnamespace Monolog\\Handler;\n\nuse Monolog\\Traits\\LoggableTrait;\n\nabstract class AbstractHandler implements HandlerInterface\n{\n    use LoggableTrait;\n\n    public function close(): void\n    {\n    }\n}\n",
    ),
    (
        "src/Monolog/Handler/StreamHandler.php",
        "<?php\n\nnamespace Monolog\\Handler;\n\nclass StreamHandler extends AbstractHandler\n{\n    public function close(): void\n    {\n        parent::close();\n    }\n\n    public function handle(array $record): bool\n    {\n        return true;\n    }\n}\n",
    ),
    (
        "src/Monolog/Handler/NullHandler.php",
        "<?php\n\nnamespace Monolog\\Handler;\n\nuse Monolog\\Handler\\HandlerInterface as Contract;\n\nclass NullHandler implements Contract\n{\n    public function handle(array $record): bool\n    {\n        return false;\n    }\n}\n",
    ),
    (
        "src/Monolog/Traits/LoggableTrait.php",
        "<?php\n\nnamespace Monolog\\Traits;\n\ntrait LoggableTrait\n{\n}\n",
    ),
    (
        "src/Monolog/Exception.php",
        "<?php\n\nnamespace Monolog;\n\nclass Exception extends \\Exception\n{\n}\n",
    ),
    (
        "src/Monolog/Psr.php",
        "<?php\n\nnamespace Monolog;\n\nuse Psr\\Log\\LoggerInterface;\n\nclass Logger implements LoggerInterface\n{\n}\n",
    ),
];

/// Newtonsoft's shape: one `base_list` names the base class and an interface
/// alike, so each entry's edge kind follows what it binds. A struct implements
/// an interface, an interface extends one, a generic base binds by its name,
/// a record's primary-constructor base is its base, an alias and a `global::`
/// name reach the reader, and a class whose one
/// supertype is an interface has no base for `base.` to reach.
pub const NEWTONSOFT: Fixture = &[
    (
        "Src/Newtonsoft.Json/JsonReader.cs",
        "using System;\n\nnamespace Newtonsoft.Json\n{\n    public abstract class JsonReader : IDisposable\n    {\n        public virtual void Close()\n        {\n        }\n    }\n}\n",
    ),
    (
        "Src/Newtonsoft.Json/IJsonLineInfo.cs",
        "namespace Newtonsoft.Json\n{\n    public interface IJsonLineInfo\n    {\n        bool HasLineInfo();\n    }\n}\n",
    ),
    (
        "Src/Newtonsoft.Json/IJsonPositionInfo.cs",
        "namespace Newtonsoft.Json;\n\npublic interface IJsonPositionInfo : IJsonLineInfo\n{\n}\n",
    ),
    (
        "Src/Newtonsoft.Json/JsonTextReader.cs",
        "namespace Newtonsoft.Json\n{\n    public class JsonTextReader : JsonReader, IJsonLineInfo\n    {\n        public override void Close()\n        {\n            base.Close();\n        }\n\n        public bool HasLineInfo()\n        {\n            return true;\n        }\n    }\n}\n",
    ),
    (
        "Src/Newtonsoft.Json/Linq/JTokenReader.cs",
        "using Newtonsoft.Json;\n\nnamespace Newtonsoft.Json.Linq\n{\n    public class JTokenReader : JsonReader, IJsonLineInfo\n    {\n        public bool HasLineInfo()\n        {\n            return false;\n        }\n    }\n}\n",
    ),
    (
        "Src/Newtonsoft.Json/JsonConverter.cs",
        "namespace Newtonsoft.Json\n{\n    public abstract class JsonConverter<T>\n    {\n    }\n\n    public struct LinePosition : IJsonLineInfo\n    {\n        public bool HasLineInfo()\n        {\n            return true;\n        }\n    }\n\n    public record JsonToken(int Depth);\n\n    public record JsonStartToken(int Depth) : JsonToken(Depth);\n}\n",
    ),
    (
        "Src/Newtonsoft.Json/Converters/IntConverter.cs",
        "using Newtonsoft.Json;\nusing Reader = Newtonsoft.Json.JsonReader;\n\nnamespace Newtonsoft.Json.Converters\n{\n    public class IntConverter : JsonConverter<int>\n    {\n    }\n\n    public class AliasedReader : Reader\n    {\n    }\n\n    public class RootedReader : global::Newtonsoft.Json.JsonReader\n    {\n    }\n\n    public class LineOnly : IJsonLineInfo\n    {\n        public bool HasLineInfo()\n        {\n            return base.HasLineInfo();\n        }\n    }\n}\n",
    ),
];

/// Kotlin supertype lists: a base class invoked beside an interface, an
/// aliased base, a delegated interface, an object, an interface extending
/// one, and a companion object whose supertype is its own — never its
/// enclosing class's. `super<Iface>` names its supertype, so the caller's
/// hierarchy does not decide it.
pub const KOTLIN: Fixture = &[
    (
        "src/main/kotlin/org/koin/core/Base.kt",
        "package org.koin.core\n\nopen class Base {\n    open fun start() {}\n}\n",
    ),
    (
        "src/main/kotlin/org/koin/core/Iface.kt",
        "package org.koin.core\n\ninterface Iface {\n    fun run() {}\n    fun start() {}\n}\n",
    ),
    (
        "src/main/kotlin/org/koin/core/Sub.kt",
        "package org.koin.core\n\ninterface Sub : Iface\n",
    ),
    (
        "src/main/kotlin/org/koin/core/impl/Impl.kt",
        "package org.koin.core.impl\n\nimport org.koin.core.Base\nimport org.koin.core.Iface\nimport org.koin.core.Base as KBase\n\nclass Impl : Base(), Iface {\n    override fun start() {\n        super.start()\n    }\n\n    fun both() {\n        super<Iface>.start()\n    }\n}\n\nclass Aliased : KBase()\n\nclass Deleg(i: Iface) : Iface by i\n\nobject Single : Iface\n\nclass Host {\n    companion object : Iface\n}\n",
    ),
];

/// A Rust trait and its impl: the impl method's `Implements` row binds the trait
/// by the S-281 rule, exactly as before — beside a Kotlin interface of the same
/// name, which a Kotlin class implements without ever reaching the trait.
pub const RUST_BESIDE_KOTLIN: Fixture = &[
    ("Cargo.toml", "[package]\nname = \"shapes\"\nversion = \"0.1.0\"\n"),
    (
        "src/lib.rs",
        "pub trait Iface {\n    fn run(&self);\n}\n\npub struct Sq;\n\nimpl Iface for Sq {\n    fn run(&self) {}\n}\n",
    ),
    (
        "src/main/kotlin/org/koin/core/Iface.kt",
        "package org.koin.core\n\ninterface Iface {\n    fun run() {}\n}\n\nclass K : Iface\n",
    ),
];
