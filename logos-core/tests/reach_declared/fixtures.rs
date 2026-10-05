//! One fixture per code language for the reach verification ([FR-PL-09]).
//!
//! Every fixture holds the same five shapes in the language's own idiom, each
//! spanning **two files** so that a plugin able to bind across files has an edge
//! to bind: a call into another file's function, an import, a class extending a
//! base declared elsewhere, a type implementing an interface declared elsewhere,
//! and a field read through a typed receiver declared elsewhere. A language that
//! binds none of them is measured against inputs that would have bound, not
//! against an empty corpus.
//!
//! [FR-PL-09]: ../../../docs/specs/requirements/FR-PL-09.md

pub type Fixture = &'static [(&'static str, &'static str)];

pub const RUST: Fixture = &[
    (
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
    ),
    (
        "src/lib.rs",
        "use crate::util::run;\n\npub fn alpha() {\n    run();\n}\n",
    ),
    ("src/util.rs", "pub fn run() {}\n"),
    (
        "src/shapes.rs",
        "pub trait Area {\n    fn area(&self) -> f64;\n}\n\npub struct Circle {\n    pub radius: f64,\n}\n",
    ),
    (
        "src/circle.rs",
        "use crate::shapes::{Area, Circle};\n\nimpl Area for Circle {\n    fn area(&self) -> f64 {\n        self.radius * self.radius\n    }\n}\n",
    ),
];

pub const JAVA: Fixture = &[
    (
        "src/main/java/com/x/Base.java",
        "package com.x;\n\npublic class Base {\n    public int count;\n\n    public void run() {}\n}\n",
    ),
    (
        "src/main/java/com/x/Shape.java",
        "package com.x;\n\npublic interface Shape {\n    int area();\n}\n",
    ),
    (
        "src/main/java/com/y/Child.java",
        "package com.y;\n\nimport com.x.Base;\nimport com.x.Shape;\n\npublic class Child extends Base implements Shape {\n    public int area() {\n        Base other = new Base();\n        other.run();\n        return other.count + count;\n    }\n}\n",
    ),
];

pub const GO: Fixture = &[
    ("go.mod", "module example.com/m\n\ngo 1.21\n"),
    (
        "util/util.go",
        "package util\n\ntype Box struct {\n\tN int\n}\n\nfunc Run() {}\n",
    ),
    (
        "main.go",
        "package main\n\nimport \"example.com/m/util\"\n\nfunc main() {\n\tutil.Run()\n\tvar b util.Box\n\t_ = b.N\n\thelper()\n}\n",
    ),
    ("helper.go", "package main\n\nfunc helper() {}\n"),
];

pub const TYPESCRIPT: Fixture = &[
    (
        "web/base.ts",
        "export interface Shape {\n  area(): number;\n}\n\nexport class Base {\n  count = 1;\n  run(): void {}\n}\n\nexport function helper(): number {\n  return 1;\n}\n",
    ),
    (
        "web/child.ts",
        "import { Base, Shape, helper } from \"./base\";\n\nexport class Child extends Base implements Shape {\n  area(): number {\n    const other: Base = new Base();\n    other.run();\n    return helper() + other.count + this.count;\n  }\n}\n",
    ),
];

pub const TSX: Fixture = &[
    (
        "web/base.tsx",
        "export interface Shape {\n  area(): number;\n}\n\nexport class Base {\n  count = 1;\n  run(): void {}\n}\n\nexport function helper(): number {\n  return 1;\n}\n",
    ),
    (
        "web/child.tsx",
        "import { Base, Shape, helper } from \"./base\";\n\nexport class Child extends Base implements Shape {\n  area(): number {\n    const other: Base = new Base();\n    other.run();\n    return helper() + other.count + this.count;\n  }\n}\n",
    ),
];

pub const PYTHON: Fixture = &[
    (
        "pkg/__init__.py",
        "",
    ),
    (
        "pkg/base.py",
        "class Base:\n    count = 1\n\n    def run(self):\n        pass\n\n\ndef helper():\n    return 1\n",
    ),
    (
        "pkg/child.py",
        "from pkg.base import Base, helper\n\n\nclass Child(Base):\n    def area(self):\n        other = Base()\n        other.run()\n        return helper() + other.count + self.count\n",
    ),
];

pub const PHP: Fixture = &[
    (
        "src/Base.php",
        "<?php\nnamespace App\\X;\n\nclass Base {\n    public int $count = 1;\n\n    public function run(): void {}\n}\n",
    ),
    (
        "src/Shape.php",
        "<?php\nnamespace App\\X;\n\ninterface Shape {\n    public function area(): int;\n}\n",
    ),
    (
        "src/Child.php",
        "<?php\nnamespace App\\Y;\n\nuse App\\X\\Base;\nuse App\\X\\Shape;\n\nclass Child extends Base implements Shape {\n    public function area(): int {\n        $other = new Base();\n        $other->run();\n        return $other->count + $this->count;\n    }\n}\n",
    ),
];

pub const C_SHARP: Fixture = &[
    (
        "X/Base.cs",
        "namespace App.X\n{\n    public class Base\n    {\n        public int Count = 1;\n\n        public void Run() { }\n    }\n}\n",
    ),
    (
        "X/IShape.cs",
        "namespace App.X\n{\n    public interface IShape\n    {\n        int Area();\n    }\n}\n",
    ),
    (
        "Y/Child.cs",
        "using App.X;\n\nnamespace App.Y\n{\n    public class Child : Base, IShape\n    {\n        public int Area()\n        {\n            Base other = new Base();\n            other.Run();\n            return other.Count + Count;\n        }\n    }\n}\n",
    ),
];

pub const KOTLIN: Fixture = &[
    (
        "src/main/kotlin/com/x/Base.kt",
        "package com.x\n\nopen class Base {\n    var count: Int = 1\n\n    fun run() {}\n}\n\ninterface Shape {\n    fun area(): Int\n}\n\nfun helper(): Int = 1\n",
    ),
    (
        "src/main/kotlin/com/y/Child.kt",
        "package com.y\n\nimport com.x.Base\nimport com.x.Shape\nimport com.x.helper\n\nclass Child : Base(), Shape {\n    override fun area(): Int {\n        val other = Base()\n        other.run()\n        return helper() + other.count + count\n    }\n}\n\nfun make() = Base()\n",
    ),
];

pub const RUBY: Fixture = &[
    (
        "lib/base.rb",
        "class Base\n  attr_reader :count\n\n  def run\n  end\nend\n\ndef helper\n  1\nend\n",
    ),
    (
        "lib/child.rb",
        "require_relative \"base\"\n\nclass Child < Base\n  def area\n    other = Base.new\n    other.run\n    helper + other.count + count\n  end\nend\n",
    ),
];

pub const SCALA: Fixture = &[
    (
        "src/main/scala/com/x/Base.scala",
        "package com.x\n\nclass Base {\n  var count: Int = 1\n\n  def run(): Unit = {}\n}\n\ntrait Shape {\n  def area(): Int\n}\n\nobject Helper {\n  def helper(): Int = 1\n}\n",
    ),
    (
        "src/main/scala/com/y/Child.scala",
        "package com.y\n\nimport com.x.Base\nimport com.x.Shape\nimport com.x.Helper.helper\n\nclass Child extends Base with Shape {\n  def area(): Int = {\n    val other = new Base()\n    other.run()\n    helper() + other.count + count\n  }\n}\n\ndef make() = Base()\n",
    ),
];

pub const C: Fixture = &[
    (
        "src/util.h",
        "int helper(void);\n\nstruct Box {\n    int count;\n};\n",
    ),
    (
        "src/util.c",
        "#include \"util.h\"\n\nint helper(void) {\n    return 1;\n}\n",
    ),
    (
        "src/main.c",
        "#include \"util.h\"\n\nstatic int twice(int x) {\n    return x + x;\n}\n\nint area(struct Box *b) {\n    return twice(helper()) + b->count;\n}\n",
    ),
];

pub const CPP: Fixture = &[
    (
        "src/base.hpp",
        "class Base {\npublic:\n    int count = 1;\n    void run() {}\n};\n\nclass Shape {\npublic:\n    virtual int area() = 0;\n};\n\nint helper();\n",
    ),
    (
        "src/base.cpp",
        "#include \"base.hpp\"\n\nint helper() {\n    return 1;\n}\n",
    ),
    (
        "src/child.cpp",
        "#include \"base.hpp\"\n\nstatic int twice(int x) {\n    return x + x;\n}\n\nclass Child : public Base, public Shape {\npublic:\n    int area() override {\n        Base other;\n        other.run();\n        return twice(helper()) + other.count + count;\n    }\n};\n",
    ),
];

/// Every fixture, by the descriptor name its language registers under.
pub const ALL: &[(&str, Fixture)] = &[
    ("rust", RUST),
    ("java", JAVA),
    ("go", GO),
    ("typescript", TYPESCRIPT),
    ("tsx", TSX),
    ("python", PYTHON),
    ("php", PHP),
    ("c-sharp", C_SHARP),
    ("kotlin", KOTLIN),
    ("ruby", RUBY),
    ("scala", SCALA),
    ("c", C),
    ("cpp", CPP),
];
