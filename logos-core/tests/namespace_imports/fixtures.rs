//! The per-language fixtures of `namespace_imports.rs` (S-518): small trees in
//! each language's own idiom, written into a temp directory per test. Each one
//! is shaped like the inspection estate it stands for — a PSR-4 PHP library, a
//! file-scoped ASP.NET Core service and a block-namespace C# library, a Kotlin
//! Multiplatform `commonMain` source set, a Scala 2 library — and each pairs the
//! case that must bind with the near miss that must not.

/// `(project-relative path, source)` pairs.
pub type Fixture = &'static [(&'static str, &'static str)];

/// A monolog-shaped PSR-4 library: an internal `use` binds the class it names,
/// a PSR interface stays unbound.
pub const PHP: Fixture = &[
    (
        "src/Monolog/Handler/HandlerInterface.php",
        "<?php\n\ndeclare(strict_types=1);\n\nnamespace Monolog\\Handler;\n\ninterface HandlerInterface\n{\n    public function handle(array $record): bool;\n}\n",
    ),
    (
        "src/Monolog/Logger.php",
        "<?php\n\ndeclare(strict_types=1);\n\nnamespace Monolog;\n\nuse Monolog\\Handler\\HandlerInterface;\nuse Psr\\Log\\LoggerInterface;\n\nclass Logger implements LoggerInterface\n{\n    /** @var HandlerInterface[] */\n    protected array $handlers = [];\n}\n",
    ),
    // A legacy, non-PSR-4 location: the declared namespace still names it.
    (
        "lib/legacy/formatters.php",
        "<?php\n\nnamespace Monolog\\Formatter;\n\nclass LineFormatter\n{\n}\n",
    ),
    (
        "src/Monolog/Handler/StreamHandler.php",
        "<?php\n\nnamespace Monolog\\Handler;\n\nuse Monolog\\Formatter\\LineFormatter;\n\nclass StreamHandler implements HandlerInterface\n{\n    public function handle(array $record): bool { return true; }\n}\n",
    ),
];

/// A C# tree mixing both namespace forms, a `global using`, a `using static`,
/// an alias, and a namespace that differs from its directory.
pub const C_SHARP: Fixture = &[
    // File-scoped, under a directory that matches.
    (
        "src/Ordering.Domain/Order.cs",
        "namespace eShop.Ordering.Domain;\n\npublic class Order\n{\n    public int Id { get; set; }\n}\n",
    ),
    // Block form, the same namespace, a directory that does not match.
    (
        "src/Shared/OrderItem.cs",
        "namespace eShop.Ordering.Domain\n{\n    public class OrderItem\n    {\n        public int Units;\n    }\n}\n",
    ),
    (
        "src/Shared/Guard.cs",
        "namespace eShop.Shared\n{\n    public static class Guard\n    {\n        public static void NotNull(object o) { }\n    }\n}\n",
    ),
    (
        "src/Ordering.API/GlobalUsings.cs",
        "global using System;\nglobal using eShop.Ordering.Domain;\n",
    ),
    (
        "src/Ordering.API/Api/OrdersApi.cs",
        "using Microsoft.AspNetCore.Mvc;\nusing static eShop.Shared.Guard;\nusing Item = eShop.Ordering.Domain.OrderItem;\n\nnamespace eShop.Ordering.API;\n\npublic class OrdersApi\n{\n    public void Get() { }\n}\n",
    ),
];

/// A Kotlin Multiplatform tree: `commonMain` and `jvmMain` sit outside every
/// `src/{main,test}/kotlin` root, and their imports bind through the `package`
/// header.
pub const KOTLIN: Fixture = &[
    (
        "core/src/commonMain/kotlin/org/koin/core/module/Module.kt",
        "package org.koin.core.module\n\nclass Module\n\nfun module(): Module = Module()\n",
    ),
    (
        "core/src/commonMain/kotlin/org/koin/core/Koin.kt",
        "package org.koin.core\n\nimport org.koin.core.module.Module\nimport org.koin.core.module.module\nimport kotlinx.coroutines.CoroutineScope\n\nclass Koin {\n    fun load(m: Module) {}\n}\n",
    ),
    (
        "core/src/jvmMain/kotlin/org/koin/core/KoinPlatform.kt",
        "package org.koin.core\n\nimport org.koin.core.module.*\n\nobject KoinPlatform\n",
    ),
];

/// A Scala 2 library: single, selector-group, wildcard, member and
/// comma-separated imports.
pub const SCALA: Fixture = &[
    (
        "core/src/main/scala/cats/data/Chain.scala",
        "package cats\npackage data\n\nclass Chain\n\nobject Chain {\n  def empty: Chain = new Chain\n}\n\ntrait Validated\n",
    ),
    (
        "core/src/main/scala/cats/Show.scala",
        "package cats\n\ntrait Show\n",
    ),
    (
        "app/src/main/scala/app/Main.scala",
        "package app\n\nimport cats.data.Chain\nimport cats.data.{Validated, Missing}\nimport cats.Show, scala.util.Try\nimport cats.data._\nimport cats.data.Chain.empty\n\nobject Main\n",
    ),
];

/// A Java tree exercising every Java import shape — single-type, static,
/// on-demand, static on-demand — and a type relation.
pub const JAVA: Fixture = &[
    (
        "svc/src/main/java/com/x/svc/Svc.java",
        "package com.x.svc;\n\npublic class Svc {\n    public static String helper() { return \"h\"; }\n    public static String util() { return \"u\"; }\n}\n",
    ),
    (
        "svc/src/main/java/com/x/svc/Base.java",
        "package com.x.svc;\n\npublic class Base {\n}\n",
    ),
    (
        "web/src/main/java/com/x/web/Ctl.java",
        "package com.x.web;\n\nimport com.x.svc.Svc;\nimport com.x.svc.*;\nimport static com.x.svc.Svc.helper;\nimport static com.x.svc.Svc.*;\nimport java.util.List;\n\npublic class Ctl extends Base {\n    private Svc svc;\n    public String a() { return helper(); }\n    public String b() { return util(); }\n}\n",
    ),
];
