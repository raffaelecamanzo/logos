//! Unit tests for the build-manifest reader (S-462).
//!
//! One fixture per reference kind, per `${…}` resolution path and per refusal,
//! per Gradle shape. The Maven fixtures are cut down from the shapes that
//! dominate the reference estate (79 poms, 2026-09-28): a service whose
//! `<parent>` is a pom in **another** member and whose dependencies carry no
//! version, and a multi-module member (`mailserver-common`) whose modules inherit
//! their group from an in-member parent that defines `${james.groupId}`.

use super::*;

fn only(facts: &[ManifestFacts], path: &str) -> ManifestFacts {
    facts
        .iter()
        .find(|m| m.path == path)
        .unwrap_or_else(|| panic!("no facts for {path}"))
        .clone()
}

fn produced(m: &ManifestFacts) -> &ArtifactFact {
    let produced: Vec<_> = m
        .artifacts
        .iter()
        .filter(|a| a.role == ArtifactRole::Produced)
        .collect();
    assert_eq!(produced.len(), 1, "exactly one produced fact per manifest");
    produced[0]
}

fn refs(m: &ManifestFacts, kind: ReferenceKind) -> Vec<&ArtifactFact> {
    m.artifacts.iter().filter(|a| a.kind == Some(kind)).collect()
}

fn key(a: &ArtifactFact) -> (Option<&str>, Option<&str>) {
    (a.group_id.as_deref(), a.artifact_id.as_deref())
}

// ── manifest identity ────────────────────────────────────────────────────────

#[test]
fn a_manifest_is_named_by_its_exact_basename_and_nothing_one_character_off() {
    assert_eq!(manifest_format("pom.xml"), Some(ManifestFormat::Maven));
    assert_eq!(manifest_format("a/b/pom.xml"), Some(ManifestFormat::Maven));
    assert_eq!(manifest_format("build.gradle"), Some(ManifestFormat::Gradle));
    assert_eq!(manifest_format("svc/build.gradle.kts"), Some(ManifestFormat::Gradle));
    for near_miss in [
        "pom.xml.bak",
        "mypom.xml",
        "pom.xm",
        "a/pom.xml/x",
        "settings.gradle",
        "settings.gradle.kts",
        "build.gradle.old",
        "xbuild.gradle",
        "Pom.xml",
    ] {
        assert_eq!(manifest_format(near_miss), None, "{near_miss} is not a manifest");
    }
}

/// The descriptors stop being module-root markers only **by addition**: every
/// manifest this reader admits is already one, and the marker list itself is
/// not edited (S-462 AC, [CR-148] §3.3).
#[test]
fn every_manifest_the_reader_admits_is_already_a_module_descriptor() {
    use crate::extract::config::corpus::MODULE_DESCRIPTORS;
    assert!(MODULE_DESCRIPTORS.contains(&MAVEN_MANIFEST));
    for gradle in GRADLE_MANIFESTS {
        assert!(MODULE_DESCRIPTORS.contains(&gradle), "{gradle} marks a module root");
    }
    assert_eq!(
        MODULE_DESCRIPTORS,
        ["pom.xml", "build.gradle", "build.gradle.kts", "package.json", "go.mod"],
        "MODULE_DESCRIPTORS is unchanged by S-462"
    );
}

// ── Maven: produced coordinates ──────────────────────────────────────────────

/// The estate's dominant shape: an explicit group, a parent pom that lives in
/// another member, and dependencies whose versions that parent manages.
const SERVICE_POM: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0">
    <modelVersion>4.0.0</modelVersion>
    <parent>
        <groupId>com.sourcesense.poste.pec</groupId>
        <artifactId>poste-pec-starter</artifactId>
        <version>2.2.1-SNAPSHOT</version>
    </parent>
    <groupId>com.sourcesense.archive</groupId>
    <artifactId>archive-manager</artifactId>
    <version>2.0.2-SNAPSHOT</version>
    <properties>
        <netty-bom.version>4.1.133.Final</netty-bom.version>
    </properties>
    <dependencies>
        <dependency>
            <groupId>com.sourcesense.poste.pec.archive</groupId>
            <artifactId>kafka-models</artifactId>
        </dependency>
        <dependency>
            <groupId>org.junit.jupiter</groupId>
            <artifactId>junit-jupiter</artifactId>
            <scope>test</scope>
            <exclusions>
                <exclusion>
                    <groupId>org.excluded</groupId>
                    <artifactId>never-a-reference</artifactId>
                </exclusion>
            </exclusions>
        </dependency>
    </dependencies>
    <build>
        <plugins>
            <plugin>
                <groupId>org.apache.maven.plugins</groupId>
                <artifactId>maven-compiler-plugin</artifactId>
                <dependencies>
                    <dependency>
                        <groupId>org.plugin</groupId>
                        <artifactId>plugin-dependency-not-a-project-reference</artifactId>
                    </dependency>
                </dependencies>
            </plugin>
        </plugins>
    </build>
    <profiles>
        <profile>
            <dependencies>
                <dependency>
                    <groupId>org.profile</groupId>
                    <artifactId>profile-only</artifactId>
                </dependency>
            </dependencies>
        </profile>
    </profiles>
</project>
"#;

#[test]
fn the_dominant_service_shape_yields_its_coordinates_parent_and_versionless_dependencies() {
    let facts = member_facts(&[("pom.xml", SERVICE_POM)]);
    let m = only(&facts, "pom.xml");
    assert_eq!((m.status, m.detail.as_deref()), (ManifestStatus::Read, None));

    let own = produced(&m);
    assert_eq!(key(own), (Some("com.sourcesense.archive"), Some("archive-manager")));
    assert_eq!(own.version.as_deref(), Some("2.0.2-SNAPSHOT"));
    assert_eq!((own.resolution, own.reason.as_deref()), (Resolution::Resolved, None));
    assert_eq!(own.kind, None, "a produced fact carries no reference kind");

    let parent = refs(&m, ReferenceKind::Parent);
    assert_eq!(parent.len(), 1);
    assert_eq!(key(parent[0]), (Some("com.sourcesense.poste.pec"), Some("poste-pec-starter")));
    assert_eq!(parent[0].version.as_deref(), Some("2.2.1-SNAPSHOT"));
    assert_eq!(parent[0].scope, None);

    let deps = refs(&m, ReferenceKind::Dependency);
    let keys: Vec<_> = deps.iter().map(|d| key(d)).collect();
    assert_eq!(
        keys,
        vec![
            (Some("com.sourcesense.poste.pec.archive"), Some("kafka-models")),
            (Some("org.junit.jupiter"), Some("junit-jupiter")),
        ],
        "only <project><dependencies> — no exclusion, plugin dependency or profile dependency"
    );
    // A version managed by the parent is absent, not refused and not guessed.
    assert_eq!(
        (deps[0].version.as_deref(), deps[0].resolution, deps[0].scope.as_deref()),
        (None, Resolution::Resolved, None),
        "an undeclared scope is recorded absent, never defaulted to compile"
    );
    assert_eq!(deps[1].scope.as_deref(), Some("test"));
    assert!(refs(&m, ReferenceKind::Managed).is_empty());
    assert!(refs(&m, ReferenceKind::BomImport).is_empty());
}

#[test]
fn a_module_without_a_group_inherits_it_and_its_version_from_parent() {
    let pom = r#"<project>
        <parent>
            <groupId>com.sourcesense.james.common</groupId>
            <artifactId>parent</artifactId>
            <version>2.1.0-SNAPSHOT</version>
        </parent>
        <artifactId>external-user-data-module</artifactId>
    </project>"#;
    let facts = member_facts(&[("external-user-data-module/pom.xml", pom)]);
    let own = produced(&facts[0]).clone();
    assert_eq!(
        key(&own),
        (Some("com.sourcesense.james.common"), Some("external-user-data-module")),
        "the group is inherited from <parent> — even when the parent is not in this member"
    );
    assert_eq!(own.version.as_deref(), Some("2.1.0-SNAPSHOT"));
    assert_eq!(own.resolution, Resolution::Resolved);
}

#[test]
fn a_pom_with_no_group_and_no_parent_is_refused_not_given_a_default_group() {
    let facts = member_facts(&[("pom.xml", "<project><artifactId>orphan</artifactId></project>")]);
    let own = produced(&facts[0]);
    assert_eq!(own.resolution, Resolution::Refused);
    assert_eq!(own.group_id, None, "an absent group is stored absent, not as \"\"");
    assert_eq!(own.artifact_id.as_deref(), Some("orphan"));
    assert!(
        own.reason.as_deref().unwrap().contains("no <groupId> and no <parent>"),
        "{:?}",
        own.reason
    );
}

// ── Maven: the four reference kinds ──────────────────────────────────────────

const MANAGED_POM: &str = r#"<project>
    <groupId>com.example</groupId>
    <artifactId>platform</artifactId>
    <version>1.0</version>
    <packaging>pom</packaging>
    <dependencyManagement>
        <dependencies>
            <dependency>
                <groupId>com.example</groupId>
                <artifactId>common</artifactId>
                <version>3.1</version>
            </dependency>
            <dependency>
                <groupId>org.springframework.cloud</groupId>
                <artifactId>spring-cloud-dependencies</artifactId>
                <version>2021.0.8</version>
                <type>pom</type>
                <scope>import</scope>
            </dependency>
        </dependencies>
    </dependencyManagement>
    <dependencies>
        <dependency>
            <groupId>com.example</groupId>
            <artifactId>runtime-lib</artifactId>
            <version>1.0</version>
            <scope>runtime</scope>
        </dependency>
    </dependencies>
</project>"#;

#[test]
fn dependency_management_is_managed_and_scope_import_is_a_bom_import() {
    let facts = member_facts(&[("pom.xml", MANAGED_POM)]);
    let m = &facts[0];

    let managed = refs(m, ReferenceKind::Managed);
    assert_eq!(managed.len(), 1, "a managed pin is not a dependency");
    assert_eq!(key(managed[0]), (Some("com.example"), Some("common")));
    assert_eq!((managed[0].version.as_deref(), managed[0].scope.as_deref()), (Some("3.1"), None));

    let bom = refs(m, ReferenceKind::BomImport);
    assert_eq!(bom.len(), 1, "scope=import in <dependencyManagement> is a BOM import");
    assert_eq!(key(bom[0]), (Some("org.springframework.cloud"), Some("spring-cloud-dependencies")));
    assert_eq!(bom[0].scope.as_deref(), Some("import"));

    let deps = refs(m, ReferenceKind::Dependency);
    assert_eq!(deps.len(), 1, "a managed entry is never also a dependency");
    assert_eq!(deps[0].scope.as_deref(), Some("runtime"));

    // Every kind is distinct, and the order is BY KIND, not by document: the
    // fixture declares <dependencyManagement> before <dependencies>, yet the
    // dependencies come first — produced, then dependencies, then management.
    let order: Vec<Option<ReferenceKind>> = m.artifacts.iter().map(|a| a.kind).collect();
    assert_eq!(
        order,
        vec![None, Some(ReferenceKind::Dependency), Some(ReferenceKind::Managed), Some(ReferenceKind::BomImport)]
    );
}

#[test]
fn the_kind_tokens_are_the_persisted_vocabulary() {
    assert_eq!(
        [ReferenceKind::Parent, ReferenceKind::Dependency, ReferenceKind::Managed, ReferenceKind::BomImport]
            .map(ReferenceKind::as_str),
        ["parent", "dependency", "managed", "bom-import"]
    );
    assert_eq!(
        [Resolution::Resolved, Resolution::VersionRefused, Resolution::Refused].map(Resolution::as_str),
        ["resolved", "version-refused", "refused"]
    );
    assert_eq!(
        [ManifestStatus::Read, ManifestStatus::Malformed, ManifestStatus::Unreadable].map(ManifestStatus::as_str),
        ["read", "malformed", "unreadable"]
    );
}

// ── Maven: `${…}` resolution and refusal ─────────────────────────────────────

#[test]
fn a_property_resolves_from_the_poms_own_properties() {
    let pom = r#"<project>
        <groupId>com.sourcesense.james.common</groupId>
        <artifactId>parent</artifactId>
        <version>2.1.0-SNAPSHOT</version>
        <properties>
            <james.groupId>org.apache.james</james.groupId>
            <james.version>3.7.4</james.version>
        </properties>
        <dependencyManagement><dependencies><dependency>
            <groupId>${james.groupId}</groupId>
            <artifactId>james-server-cli</artifactId>
            <version>${james.version}</version>
        </dependency></dependencies></dependencyManagement>
    </project>"#;
    let facts = member_facts(&[("pom.xml", pom)]);
    let managed = refs(&facts[0], ReferenceKind::Managed)[0].clone();
    assert_eq!(key(&managed), (Some("org.apache.james"), Some("james-server-cli")));
    assert_eq!(managed.version.as_deref(), Some("3.7.4"));
    assert_eq!((managed.resolution, managed.reason), (Resolution::Resolved, None));
}

#[test]
fn a_property_resolves_through_the_in_member_parent_chain_nearest_first() {
    let root = r#"<project>
        <groupId>com.example</groupId>
        <artifactId>root</artifactId>
        <version>1.0</version>
        <properties>
            <lib.group>com.from.root</lib.group>
            <lib.version>9.9</lib.version>
        </properties>
    </project>"#;
    // The middle pom sits in a subdirectory and overrides one property.
    let middle = r#"<project>
        <parent><groupId>com.example</groupId><artifactId>root</artifactId><version>1.0</version></parent>
        <artifactId>middle</artifactId>
        <properties><lib.version>2.0</lib.version></properties>
    </project>"#;
    let leaf = r#"<project>
        <parent>
            <groupId>com.example</groupId>
            <artifactId>middle</artifactId>
            <version>1.0</version>
        </parent>
        <artifactId>leaf</artifactId>
        <dependencies><dependency>
            <groupId>${lib.group}</groupId>
            <artifactId>lib</artifactId>
            <version>${lib.version}</version>
        </dependency></dependencies>
    </project>"#;
    let facts = member_facts(&[
        ("pom.xml", root),
        ("middle/pom.xml", middle),
        ("middle/leaf/pom.xml", leaf),
    ]);
    let m = only(&facts, "middle/leaf/pom.xml");
    let dep = refs(&m, ReferenceKind::Dependency)[0];
    assert_eq!(key(dep), (Some("com.from.root"), Some("lib")), "the grandparent defines the group");
    assert_eq!(dep.version.as_deref(), Some("2.0"), "the nearer parent's definition wins");
    assert_eq!(dep.resolution, Resolution::Resolved);
    assert_eq!(key(produced(&m)), (Some("com.example"), Some("leaf")));
}

#[test]
fn an_undefined_property_is_refused_with_a_reason_naming_it_and_the_out_of_member_parent() {
    let pom = r#"<project>
        <parent>
            <groupId>org.springframework.boot</groupId>
            <artifactId>spring-boot-starter-parent</artifactId>
            <version>2.7.18</version>
            <relativePath/>
        </parent>
        <groupId>com.example</groupId>
        <artifactId>svc</artifactId>
        <dependencies><dependency>
            <groupId>${external.group}</groupId>
            <artifactId>lib</artifactId>
        </dependency></dependencies>
    </project>"#;
    let facts = member_facts(&[("pom.xml", pom)]);
    let dep = refs(&facts[0], ReferenceKind::Dependency)[0].clone();
    assert_eq!(dep.resolution, Resolution::Refused, "an unresolved group refuses the whole fact");
    assert_eq!(
        dep.group_id.as_deref(),
        Some("${external.group}"),
        "the declared text is kept verbatim — never replaced by a guess"
    );
    let reason = dep.reason.expect("a refusal carries its reason");
    assert!(reason.starts_with("groupId: `${external.group}` is not defined"), "{reason}");
    assert!(
        reason.contains("parent `org.springframework.boot:spring-boot-starter-parent` is not a pom of this member"),
        "{reason}"
    );
}

#[test]
fn an_unresolved_version_keeps_the_key_and_is_version_refused() {
    // The estate's most frequent property: a version the out-of-member parent
    // defines. The artifact is still named; only its version is unknown.
    let pom = r#"<project>
        <parent><groupId>com.sourcesense.poste.pec</groupId><artifactId>poste-pec-starter</artifactId><version>2.2.1</version></parent>
        <artifactId>svc</artifactId>
        <dependencies><dependency>
            <groupId>org.projectlombok</groupId>
            <artifactId>lombok</artifactId>
            <version>${lombok.version}</version>
        </dependency></dependencies>
    </project>"#;
    let facts = member_facts(&[("pom.xml", pom)]);
    let dep = refs(&facts[0], ReferenceKind::Dependency)[0].clone();
    assert_eq!(dep.resolution, Resolution::VersionRefused);
    assert_eq!(key(&dep), (Some("org.projectlombok"), Some("lombok")));
    assert_eq!(dep.version.as_deref(), Some("${lombok.version}"));
    assert!(dep.reason.unwrap().starts_with("version: `${lombok.version}` is not defined"));
}

#[test]
fn project_model_expressions_resolve_from_the_pom_itself() {
    let pom = r#"<project>
        <parent><groupId>com.example</groupId><artifactId>ext-parent</artifactId><version>7</version></parent>
        <artifactId>svc</artifactId>
        <dependencies>
            <dependency>
                <groupId>${project.groupId}</groupId>
                <artifactId>${project.artifactId}-api</artifactId>
                <version>${project.version}</version>
            </dependency>
            <dependency>
                <groupId>${project.parent.groupId}</groupId>
                <artifactId>sibling</artifactId>
                <version>${project.parent.version}</version>
            </dependency>
        </dependencies>
    </project>"#;
    let facts = member_facts(&[("pom.xml", pom)]);
    let deps = refs(&facts[0], ReferenceKind::Dependency);
    assert_eq!(key(deps[0]), (Some("com.example"), Some("svc-api")));
    assert_eq!(deps[0].version.as_deref(), Some("7"), "project.version inherits from <parent>");
    assert_eq!(key(deps[1]), (Some("com.example"), Some("sibling")));
    assert_eq!(deps[1].version.as_deref(), Some("7"));
    assert!(deps.iter().all(|d| d.resolution == Resolution::Resolved));
}

#[test]
fn a_property_defined_in_terms_of_itself_is_refused_not_looped() {
    let pom = r#"<project>
        <groupId>com.example</groupId>
        <artifactId>svc</artifactId>
        <properties><a>${b}</a><b>${a}</b></properties>
        <dependencies><dependency>
            <groupId>${a}</groupId><artifactId>x</artifactId>
        </dependency></dependencies>
    </project>"#;
    let facts = member_facts(&[("pom.xml", pom)]);
    let dep = refs(&facts[0], ReferenceKind::Dependency)[0];
    assert_eq!(dep.resolution, Resolution::Refused);
    assert!(dep.reason.as_deref().unwrap().contains("defined in terms of itself"), "{:?}", dep.reason);
}

#[test]
fn a_property_value_is_itself_interpolated() {
    let pom = r#"<project>
        <groupId>com.example</groupId>
        <artifactId>svc</artifactId>
        <properties><base>com.example</base><models.group>${base}.models</models.group></properties>
        <dependencies><dependency>
            <groupId>${models.group}</groupId><artifactId>kafka-models</artifactId>
        </dependency></dependencies>
    </project>"#;
    let facts = member_facts(&[("pom.xml", pom)]);
    assert_eq!(
        key(refs(&facts[0], ReferenceKind::Dependency)[0]),
        (Some("com.example.models"), Some("kafka-models"))
    );
}

#[test]
fn two_member_poms_with_the_parents_coordinates_are_told_apart_by_relative_path_or_not_at_all() {
    let twin = |group: &str| {
        format!(
            "<project><groupId>com.example</groupId><artifactId>parent</artifactId>\
             <properties><g>{group}</g></properties></project>"
        )
    };
    let (a, b) = (twin("from.a"), twin("from.b"));
    let child = |rel: &str| {
        format!(
            "<project><parent><groupId>com.example</groupId><artifactId>parent</artifactId>{rel}</parent>\
             <artifactId>child</artifactId>\
             <dependencies><dependency><groupId>${{g}}</groupId><artifactId>x</artifactId></dependency></dependencies>\
             </project>"
        )
    };

    // Default relativePath `../pom.xml` singles out `a/pom.xml`.
    let by_default = child("");
    let facts = member_facts(&[("a/pom.xml", &a), ("b/pom.xml", &b), ("a/child/pom.xml", &by_default)]);
    let dep = refs(&only(&facts, "a/child/pom.xml"), ReferenceKind::Dependency)[0].clone();
    assert_eq!(dep.group_id.as_deref(), Some("from.a"));

    // An explicit relativePath to a directory selects the other twin.
    let explicit = child("<relativePath>../../b</relativePath>");
    let facts = member_facts(&[("a/pom.xml", &a), ("b/pom.xml", &b), ("a/child/pom.xml", &explicit)]);
    let dep = refs(&only(&facts, "a/child/pom.xml"), ReferenceKind::Dependency)[0].clone();
    assert_eq!(dep.group_id.as_deref(), Some("from.b"));

    // Neither twin at the relative path: ambiguous, so nothing is inherited.
    let facts = member_facts(&[("a/pom.xml", &a), ("b/pom.xml", &b), ("c/child/pom.xml", &by_default)]);
    let dep = refs(&only(&facts, "c/child/pom.xml"), ReferenceKind::Dependency)[0].clone();
    assert_eq!(dep.resolution, Resolution::Refused, "an ambiguous parent supplies no property");
}

#[test]
fn a_parent_cycle_terminates_and_refuses_what_it_cannot_define() {
    let a = "<project><parent><groupId>g</groupId><artifactId>b</artifactId></parent>\
             <groupId>g</groupId><artifactId>a</artifactId>\
             <dependencies><dependency><groupId>${none}</groupId><artifactId>x</artifactId></dependency></dependencies></project>";
    let b = "<project><parent><groupId>g</groupId><artifactId>a</artifactId></parent>\
             <groupId>g</groupId><artifactId>b</artifactId></project>";
    let facts = member_facts(&[("a/pom.xml", a), ("b/pom.xml", b)]);
    let dep = refs(&only(&facts, "a/pom.xml"), ReferenceKind::Dependency)[0].clone();
    assert_eq!(dep.resolution, Resolution::Refused);
}

// ── Maven: well-formedness ───────────────────────────────────────────────────

#[test]
fn malformed_xml_is_recorded_malformed_with_no_facts() {
    let cases = [
        ("truncated", "<project><groupId>g</groupId><artifactId>a"),
        ("mismatched", "<project><groupId>g</artifactId></project>"),
        ("not a pom", "<settings><servers/></settings>"),
        ("two roots", "<project/><project/>"),
        ("empty", ""),
    ];
    for (label, text) in cases {
        let facts = member_facts(&[("pom.xml", text)]);
        assert_eq!(facts.len(), 1, "{label}: the manifest is still counted");
        assert_eq!(facts[0].status, ManifestStatus::Malformed, "{label}");
        assert!(facts[0].detail.is_some(), "{label}: a malformed pom says why");
        assert!(facts[0].artifacts.is_empty(), "{label}: no fact from a pom that did not parse");
    }
}

#[test]
fn entity_and_character_references_decode_and_comments_do_not_leak() {
    let pom = "<project><groupId>com.a&amp;b</groupId><artifactId>x&#45;y<!-- c --></artifactId>\
               <version><![CDATA[1.0]]></version></project>";
    let facts = member_facts(&[("pom.xml", pom)]);
    let own = produced(&facts[0]);
    assert_eq!(key(own), (Some("com.a&b"), Some("x-y")));
    assert_eq!(own.version.as_deref(), Some("1.0"));
}

#[test]
fn a_malformed_parent_leaves_its_children_refusing_rather_than_guessing() {
    let facts = member_facts(&[
        ("pom.xml", "<project><groupId>g</groupId><artifactId>root"),
        (
            "m/pom.xml",
            "<project><parent><groupId>g</groupId><artifactId>root</artifactId></parent>\
             <artifactId>m</artifactId><version>${root.version}</version></project>",
        ),
    ]);
    assert_eq!(only(&facts, "pom.xml").status, ManifestStatus::Malformed);
    let own = produced(&only(&facts, "m/pom.xml")).clone();
    assert_eq!(key(&own), (Some("g"), Some("m")));
    assert_eq!(own.resolution, Resolution::VersionRefused);
}

#[test]
fn facts_are_sorted_by_path_and_non_manifests_are_skipped() {
    let facts = member_facts(&[
        ("z/pom.xml", "<project><groupId>g</groupId><artifactId>z</artifactId></project>"),
        ("README.md", "# not a manifest"),
        ("a/build.gradle", "group = 'g'"),
    ]);
    let paths: Vec<&str> = facts.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(paths, vec!["a/build.gradle", "z/pom.xml"]);
}

// ── Gradle (fixture-pinned, unexercised on any estate) ───────────────────────

const GROOVY_BUILD: &str = r#"
plugins {
    id 'java-library'
}

group = 'com.example.lib'
version = '1.2.0'

// implementation 'commented:out:1.0'
/* testImplementation 'block:commented:1.0' */

dependencies {
    implementation 'org.springframework.boot:spring-boot-starter-web:2.7.18'
    api "com.example:models"
    implementation project(':core')
    implementation platform('org.springframework.boot:spring-boot-dependencies:2.7.18')
    testImplementation("org.junit.jupiter:junit-jupiter:$junitVersion")
    runtimeOnly "com.example:$artifactName:1.0"
    implementation files('libs/local.jar')
    implementation libs.guava
    implementation group: 'map.form', name: 'not-read', version: '1'
    implementation('com.example:with-closure:1.0') {
        exclude group: 'excluded.group', module: 'never-a-reference'
    }
}

repositories { mavenCentral() }
"#;

#[test]
fn gradle_groovy_reads_literal_coordinates_projects_platforms_and_the_group() {
    let facts = member_facts(&[("build.gradle", GROOVY_BUILD)]);
    let m = only(&facts, "build.gradle");
    assert_eq!((m.format, m.status), (ManifestFormat::Gradle, ManifestStatus::Read));

    let own = produced(&m);
    assert_eq!(own.group_id.as_deref(), Some("com.example.lib"), "the project group is read");
    assert_eq!(own.version.as_deref(), Some("1.2.0"));
    assert_eq!(own.artifact_id, None, "the artifact name is not guessed from the directory");
    assert_eq!(own.resolution, Resolution::Refused);
    assert!(own.reason.as_deref().unwrap().contains("settings.gradle"), "{:?}", own.reason);

    type Row<'a> = (Option<ReferenceKind>, Option<&'a str>, Option<&'a str>, Option<&'a str>, Option<&'a str>, Resolution);
    let got: Vec<Row<'_>> = m
        .artifacts
        .iter()
        .filter(|a| a.role == ArtifactRole::Referenced)
        .map(|a| {
            (
                a.kind,
                a.group_id.as_deref().or(a.project_path.as_deref()),
                a.artifact_id.as_deref(),
                a.version.as_deref(),
                a.scope.as_deref(),
                a.resolution,
            )
        })
        .collect();
    use ReferenceKind::{BomImport, Dependency};
    use Resolution::{Refused, Resolved, VersionRefused};
    assert_eq!(
        got,
        vec![
            (Some(Dependency), Some("org.springframework.boot"), Some("spring-boot-starter-web"), Some("2.7.18"), Some("implementation"), Resolved),
            (Some(Dependency), Some("com.example"), Some("models"), None, Some("api"), Resolved),
            (Some(Dependency), Some(":core"), None, None, Some("implementation"), Resolved),
            (Some(BomImport), Some("org.springframework.boot"), Some("spring-boot-dependencies"), Some("2.7.18"), Some("implementation"), Resolved),
            (Some(Dependency), Some("org.junit.jupiter"), Some("junit-jupiter"), Some("$junitVersion"), Some("testImplementation"), VersionRefused),
            (Some(Dependency), Some("com.example"), Some("$artifactName"), Some("1.0"), Some("runtimeOnly"), Refused),
            (Some(Dependency), Some("com.example"), Some("with-closure"), Some("1.0"), Some("implementation"), Resolved),
        ],
        "comments, files(), catalogs, map notation and closure exclusions are not references"
    );
    let project = m.artifacts.iter().find(|a| a.project_path.is_some()).unwrap();
    assert_eq!((project.group_id.as_deref(), project.artifact_id.as_deref()), (None, None));
}

const KOTLIN_BUILD: &str = r#"
plugins { `java-library` }

group = "com.example.kts"

dependencies {
    implementation("com.example:models:2.0")
    implementation(project(":shared"))
    api(enforcedPlatform("com.example:bom:5"))
    testImplementation(kotlin("test"))
    implementation(project(path = ":named"))
}
"#;

#[test]
fn gradle_kotlin_dsl_reads_the_same_shapes_with_parentheses() {
    let facts = member_facts(&[("svc/build.gradle.kts", KOTLIN_BUILD)]);
    let m = only(&facts, "svc/build.gradle.kts");
    assert_eq!(produced(&m).group_id.as_deref(), Some("com.example.kts"));
    let got: Vec<_> = m
        .artifacts
        .iter()
        .filter(|a| a.role == ArtifactRole::Referenced)
        .map(|a| (a.kind, a.group_id.as_deref(), a.artifact_id.as_deref(), a.project_path.as_deref(), a.scope.as_deref()))
        .collect();
    assert_eq!(
        got,
        vec![
            (Some(ReferenceKind::Dependency), Some("com.example"), Some("models"), None, Some("implementation")),
            (Some(ReferenceKind::Dependency), None, None, Some(":shared"), Some("implementation")),
            (Some(ReferenceKind::BomImport), Some("com.example"), Some("bom"), None, Some("api")),
            (Some(ReferenceKind::Dependency), None, None, Some(":named"), Some("implementation")),
        ]
    );
}

#[test]
fn gradle_reads_one_line_blocks_allprojects_group_and_buildscript_classpath() {
    let script = "buildscript { dependencies { classpath 'com.example:plugin:1.0' } }\n\
                  allprojects { group = 'com.example.all' }\n\
                  subprojects { group = 'com.example.sub' }\n\
                  dependencies { implementation 'a:b:1' }\n";
    let facts = member_facts(&[("build.gradle", script)]);
    let m = &facts[0];
    assert_eq!(
        produced(m).group_id.as_deref(),
        Some("com.example.all"),
        "allprojects sets the root project's group; subprojects does not"
    );
    let refs: Vec<_> = m
        .artifacts
        .iter()
        .filter(|a| a.role == ArtifactRole::Referenced)
        .map(|a| (a.artifact_id.as_deref(), a.scope.as_deref()))
        .collect();
    assert_eq!(refs, vec![(Some("plugin"), Some("classpath")), (Some("b"), Some("implementation"))]);
}

#[test]
fn a_gradle_script_with_no_literal_group_says_so() {
    let facts = member_facts(&[("build.gradle", "group = \"${rootGroup}\"\n")]);
    let own = produced(&facts[0]);
    assert_eq!(own.resolution, Resolution::Refused);
    assert!(own.reason.as_deref().unwrap().contains("interpolates"), "{:?}", own.reason);

    let facts = member_facts(&[("build.gradle", "plugins { id 'java' }\n")]);
    let own = produced(&facts[0]);
    assert_eq!(own.group_id, None);
    assert!(own.reason.as_deref().unwrap().contains("no literal `group`"), "{:?}", own.reason);
}

#[test]
fn a_gradle_near_miss_is_not_a_coordinate() {
    let script = "dependencies {\n\
                  implementation 'just-one-part'\n\
                  implementation ':missing-group:1'\n\
                  implementation 'a:b:c:d:e'\n\
                  groupId = 'not.a.dependency'\n\
                  implementation 'http://example.com/a:b'\n\
                  }\n\
                  groupId = 'not.the.group'\n";
    let facts = member_facts(&[("build.gradle", script)]);
    let m = &facts[0];
    let refs: Vec<_> = m
        .artifacts
        .iter()
        .filter(|a| a.role == ArtifactRole::Referenced)
        .map(|a| (a.group_id.as_deref(), a.artifact_id.as_deref()))
        .collect();
    assert_eq!(
        refs,
        Vec::<(Option<&str>, Option<&str>)>::new(),
        "only a 2–4 part literal with a group and an artifact, and no slash or space, is read"
    );
    assert_eq!(produced(m).group_id, None, "`groupId =` is not `group =`");
}

// ── Bounded on untrusted input (S-462 review) ────────────────────────────────

/// A pom whose `<version>` is `${p0}`, with `<p{i}>` defined by `body(i)`.
fn property_chain(depth: usize, body: impl Fn(usize) -> String) -> String {
    let props: String = (0..depth).map(|i| format!("<p{i}>{}</p{i}>", body(i))).collect();
    format!(
        "<project><groupId>g</groupId><artifactId>a</artifactId><version>${{p0}}</version>\
         <properties>{props}<p{depth}>x</p{depth}></properties></project>"
    )
}

fn produced_version_refusal(pom: &str) -> String {
    let facts = member_facts(&[("pom.xml", pom)]);
    let own = produced(&facts[0]).clone();
    assert_eq!(own.resolution, Resolution::VersionRefused, "the version is refused, the key kept");
    own.reason.expect("a refusal carries its reason")
}

/// A linear chain recurses once per link. 5000 links overflowed the 2 MiB
/// stack every test (and the watcher's sync) thread runs on, aborting the
/// process; now it is refused at the nesting bound.
#[test]
fn a_linear_property_chain_is_refused_at_the_nesting_bound_not_by_the_stack() {
    let pom = property_chain(5000, |i| format!("${{p{}}}", i + 1));
    let reason = produced_version_refusal(&pom);
    assert!(reason.contains("deeper than 32"), "{reason}");
}

/// Each property referencing the next twice doubles the expansion per level —
/// the `<properties>` form of an entity bomb. Depth 30 would have built a
/// 1 GiB string; it is refused at the length bound in a few hundred steps.
#[test]
fn a_doubling_property_chain_is_refused_rather_than_expanded() {
    let pom = property_chain(30, |i| format!("${{p{0}}}${{p{0}}}", i + 1));
    let reason = produced_version_refusal(&pom);
    assert!(
        reason.contains("expands beyond 1024 bytes") || reason.contains("more than 256"),
        "{reason}"
    );
}

/// A fan-out of EMPTY values never grows the output, so only the reference
/// budget stops it: 20 references per level over 10 levels is 20^10 expansions.
#[test]
fn a_wide_fan_out_of_empty_properties_is_refused_by_the_reference_budget() {
    let pom = property_chain(10, |i| format!("${{p{}}}", i + 1).repeat(20));
    let pom = pom.replace("<p10>x</p10>", "<p10></p10>");
    let reason = produced_version_refusal(&pom);
    assert!(reason.contains("more than 256 property references"), "{reason}");
}

/// A multi-byte character right before a `{` — here inside a `'''` string,
/// whose body the line reader scans as code — once panicked slicing inside it,
/// on the indexing thread, for every later index of the member.
#[test]
fn a_multi_byte_character_before_a_brace_does_not_panic_the_gradle_reader() {
    let script = "description = '''\nsee the docs—here {\n'''\n}\ngroup = 'com.example'\n\
                  dependencies {\n  implementation 'a:b:1'\n}\n";
    let facts = member_facts(&[("build.gradle", script)]);
    let refs: Vec<_> = facts[0]
        .artifacts
        .iter()
        .filter(|a| a.role == ArtifactRole::Referenced)
        .map(|a| (a.group_id.as_deref(), a.artifact_id.as_deref()))
        .collect();
    assert_eq!(refs, vec![(Some("a"), Some("b"))]);
    for name in ["§{", "→ {", "x✔{", "“name”{"] {
        let facts = member_facts(&[("build.gradle", name)]);
        assert_eq!(facts[0].status, ManifestStatus::Read, "{name}");
    }
}

/// The five XML entities decode; a DTD-declared one is not expanded, so the
/// pom is malformed with the entity named — `&g;` never becomes a coordinate.
#[test]
fn a_dtd_entity_makes_the_pom_malformed_rather_than_a_literal_coordinate() {
    let pom = "<project><groupId>a&lt;&gt;&apos;&quot;b</groupId><artifactId>x</artifactId></project>";
    let facts = member_facts(&[("pom.xml", pom)]);
    assert_eq!(key(produced(&facts[0])), (Some("a<>'\"b"), Some("x")));

    let pom = "<!DOCTYPE project [<!ENTITY g \"org.example\">]>\
               <project><groupId>&g;</groupId><artifactId>x</artifactId></project>";
    let facts = member_facts(&[("pom.xml", pom)]);
    assert_eq!(facts[0].status, ManifestStatus::Malformed);
    assert!(facts[0].artifacts.is_empty(), "no fact is built from an unexpanded entity");
    assert!(facts[0].detail.as_deref().unwrap().contains("`&g;` is not expanded"), "{:?}", facts[0].detail);
}

/// A key that interpolates to an empty value is refused — an empty group
/// would be a join key every other empty group matches. An empty VERSION part
/// is legitimate (`1.0${suffix}`) and still resolves.
#[test]
fn a_key_that_resolves_to_empty_is_refused_but_an_empty_version_part_is_not() {
    let pom = r#"<project>
        <groupId>com.example</groupId>
        <artifactId>svc</artifactId>
        <properties><g/><suffix/></properties>
        <dependencies>
            <dependency><groupId>${g}</groupId><artifactId>x</artifactId></dependency>
            <dependency><groupId>com.example</groupId><artifactId>y</artifactId><version>1.0${suffix}</version></dependency>
        </dependencies>
    </project>"#;
    let facts = member_facts(&[("pom.xml", pom)]);
    let deps = refs(&facts[0], ReferenceKind::Dependency);
    assert_eq!(deps[0].resolution, Resolution::Refused);
    assert_eq!(deps[0].group_id.as_deref(), Some("${g}"), "the declared text is kept");
    assert!(deps[0].reason.as_deref().unwrap().contains("resolves to an empty value"), "{:?}", deps[0].reason);
    assert_eq!((deps[1].version.as_deref(), deps[1].resolution), (Some("1.0"), Resolution::Resolved));
}
