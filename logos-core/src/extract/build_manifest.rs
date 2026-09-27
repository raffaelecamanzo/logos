//! Build manifests → member-local **artifact facts** (S-462, [CR-148] §3.2 A,
//! [ADR-69] decision point 1).
//!
//! A member's Maven `pom.xml` and Gradle `build.gradle` / `build.gradle.kts`
//! files state, in committed text, which artifacts the member **produces** and
//! which artifacts it **references**. This module reads those two facts and
//! nothing else. It owns no workspace relation: joining a reference in one member
//! to the producer in another is the federation's work ([ADR-69] point 2), and a
//! build edge is never a runtime coupling ([BR-58]).
//!
//! # What is read
//!
//! - **Maven.** The project's own `groupId` / `artifactId` / `version` — the
//!   `groupId` and `version` inherited from `<parent>` when the module declares
//!   none — and four reference kinds: `<parent>` ([`ReferenceKind::Parent`]),
//!   `<dependencies>` ([`ReferenceKind::Dependency`]),
//!   `<dependencyManagement>` ([`ReferenceKind::Managed`]) and a managed entry
//!   with `<scope>import</scope>` ([`ReferenceKind::BomImport`]). Each reference
//!   carries its declared scope. Only those exact element paths are read:
//!   `<build><plugins>` dependencies, `<exclusions>` and `<profiles>` are not
//!   project dependencies and are skipped structurally, by path.
//! - **Gradle.** Literal `"group:artifact[:version]"` strings, `project(':x')`
//!   references and `platform(…)` / `enforcedPlatform(…)` BOM imports inside a
//!   `dependencies { }` block, each with its configuration name as its scope; and
//!   the project's literal `group` (and `version`). No estate this project
//!   measures against builds with Gradle, so this half is fixture-pinned and
//!   reported **unexercised** ([CR-148] CRA-03).
//!
//! # Resolved, or refused with a reason — never guessed ([NFR-RA-05])
//!
//! A `${…}` in a Maven coordinate is interpolated from the pom's own
//! `<properties>`, then from the `<properties>` of its **in-member** parent chain
//! (nearest first), plus the `project.*` model expressions the pom itself answers
//! (`project.groupId`, `project.version`, …). A property defined nowhere in that
//! chain — typically because the parent is a pom in *another* member, or an
//! external one — is not looked up anywhere else and is not defaulted: the
//! reference is recorded **refused**, with a reason naming the property and,
//! when it applies, the out-of-member parent that might have defined it.
//!
//! The refusal is graded, because the fields do different work downstream. The
//! `groupId:artifactId` pair is the join key, so an unresolved group or artifact
//! refuses the whole fact ([`Resolution::Refused`]). An unresolved **version**
//! leaves the key intact and is recorded [`Resolution::VersionRefused`]: the
//! reference still names what it builds against, and the version it would pin is
//! stated as unknown rather than dropped. A field that did not resolve keeps its
//! declared text verbatim — `${james.version}` — so the row describes itself;
//! the resolution and reason, not the text, say whether it is a coordinate.
//!
//! Gradle's interpolation (`"g:a:$v"`) reads project properties this module does
//! not evaluate, so any `$` in a Gradle coordinate refuses it the same way. A
//! Gradle project's **artifact name** comes from `settings.gradle` or its
//! directory; neither is read, so a Gradle manifest's produced fact is always
//! recorded refused, carrying the `group` it did read.
//!
//! # Bounded on untrusted input
//!
//! A manifest is committed text anyone can write. quick-xml expands no DTD
//! entity, and interpolation is bounded three ways — nesting depth, the number
//! of references one field expands, and the expanded length — so a crafted
//! `<properties>` chain is refused with a reason instead of exhausting the stack,
//! the clock or memory.
//!
//! # A pure function of the manifest texts
//!
//! [`member_facts`] takes every manifest of one member as `(path, text)` pairs
//! and opens nothing — the [`pipeline`](crate::pipeline) reads the files, and a
//! manifest it cannot read is recorded through [`ManifestFacts::unreadable`].
//! The parent chain crosses files, so the facts of one pom depend on its
//! in-member ancestors: the pipeline therefore re-derives a member's facts
//! wholesale whenever any of its manifests changes, and does nothing at all when
//! none did.
//!
//! [ADR-69]: ../../../docs/specs/architecture/decisions/ADR-69.md
//! [BR-58]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [CR-148]: ../../../docs/requests/CR-148-build-manifests-yield-a-build-dependency-relation.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::{BTreeMap, BTreeSet};

use quick_xml::events::Event;
use quick_xml::Reader;

/// The Maven manifest basename.
pub const MAVEN_MANIFEST: &str = "pom.xml";

/// The Gradle manifest basenames, Groovy and Kotlin DSL.
pub const GRADLE_MANIFESTS: [&str; 2] = ["build.gradle", "build.gradle.kts"];

/// Which build tool a manifest belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ManifestFormat {
    /// A Maven `pom.xml`.
    Maven,
    /// A Gradle `build.gradle` or `build.gradle.kts`.
    Gradle,
}

impl ManifestFormat {
    /// The persisted token (`build_manifests.format`).
    pub fn as_str(self) -> &'static str {
        match self {
            ManifestFormat::Maven => "maven",
            ManifestFormat::Gradle => "gradle",
        }
    }
}

/// The manifest format a project-relative `path` names, by exact basename, or
/// `None` for every other file (`pom.xml.bak`, `settings.gradle`, `mypom.xml`).
pub fn manifest_format(path: &str) -> Option<ManifestFormat> {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name == MAVEN_MANIFEST {
        Some(ManifestFormat::Maven)
    } else if GRADLE_MANIFESTS.contains(&name) {
        Some(ManifestFormat::Gradle)
    } else {
        None
    }
}

/// Whether a manifest could be read at all — the numerator of the "files read,
/// of manifests found" denominator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestStatus {
    /// Read and parsed; its facts are in [`ManifestFacts::artifacts`].
    Read,
    /// Read, but not well-formed XML (or not a `<project>`); it yields no facts.
    Malformed,
    /// Could not be read from disk as UTF-8; it yields no facts.
    Unreadable,
}

impl ManifestStatus {
    /// The persisted token (`build_manifests.status`).
    pub fn as_str(self) -> &'static str {
        match self {
            ManifestStatus::Read => "read",
            ManifestStatus::Malformed => "malformed",
            ManifestStatus::Unreadable => "unreadable",
        }
    }
}

/// Whether a fact is an artifact the member builds, or one it builds against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactRole {
    /// The member's own artifact.
    Produced,
    /// An artifact the member's build names.
    Referenced,
}

impl ArtifactRole {
    /// The persisted token (`build_artifacts.role`).
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactRole::Produced => "produced",
            ArtifactRole::Referenced => "referenced",
        }
    }
}

/// How a manifest references an artifact ([CR-148] §3.2 A).
///
/// [CR-148]: ../../../docs/requests/CR-148-build-manifests-yield-a-build-dependency-relation.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceKind {
    /// Maven `<parent>`: the member inherits this pom's configuration.
    Parent,
    /// Maven `<dependencies>`, or a Gradle dependency declaration.
    Dependency,
    /// Maven `<dependencyManagement>`: a version pin, not a dependency.
    Managed,
    /// A managed entry with `scope=import`, or a Gradle `platform(…)`.
    BomImport,
}

impl ReferenceKind {
    /// The persisted token (`build_artifacts.kind`).
    pub fn as_str(self) -> &'static str {
        match self {
            ReferenceKind::Parent => "parent",
            ReferenceKind::Dependency => "dependency",
            ReferenceKind::Managed => "managed",
            ReferenceKind::BomImport => "bom-import",
        }
    }
}

/// Whether a fact's coordinates resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Every declared field resolved.
    Resolved,
    /// `groupId:artifactId` resolved; the declared version did not.
    VersionRefused,
    /// The group or artifact did not resolve — the fact names no artifact.
    Refused,
}

impl Resolution {
    /// The persisted token (`build_artifacts.resolution`).
    pub fn as_str(self) -> &'static str {
        match self {
            Resolution::Resolved => "resolved",
            Resolution::VersionRefused => "version-refused",
            Resolution::Refused => "refused",
        }
    }
}

/// One produced or referenced artifact of one manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactFact {
    /// Produced by this member, or referenced by it.
    pub role: ArtifactRole,
    /// How it is referenced; `None` exactly when [`role`](Self::role) is
    /// [`ArtifactRole::Produced`].
    pub kind: Option<ReferenceKind>,
    /// The group: resolved, or its declared text when it did not resolve.
    pub group_id: Option<String>,
    /// The artifact: resolved, or its declared text when it did not resolve.
    pub artifact_id: Option<String>,
    /// The version: resolved, declared text when it did not, `None` when none
    /// is declared (a Maven dependency whose version is managed elsewhere).
    pub version: Option<String>,
    /// The declared scope (Maven `<scope>`, a Gradle configuration name), or
    /// `None` when none is declared. Recorded as written, never defaulted.
    pub scope: Option<String>,
    /// A Gradle `project(':x')` path; `None` for every coordinate reference.
    pub project_path: Option<String>,
    /// Whether the coordinates resolved.
    pub resolution: Resolution,
    /// Why they did not; `None` exactly when [`resolution`](Self::resolution)
    /// is [`Resolution::Resolved`].
    pub reason: Option<String>,
}

/// Everything one manifest file yields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestFacts {
    /// Project-relative path of the manifest.
    pub path: String,
    /// Maven or Gradle.
    pub format: ManifestFormat,
    /// Whether it could be read.
    pub status: ManifestStatus,
    /// Why it could not be read or parsed; `None` exactly when
    /// [`status`](Self::status) is [`ManifestStatus::Read`].
    pub detail: Option<String>,
    /// Produced facts first, then references in document order.
    pub artifacts: Vec<ArtifactFact>,
}

impl ManifestFacts {
    /// The record of a manifest the caller found but could not read — counted
    /// in the "manifests found" denominator, contributing no fact.
    pub fn unreadable(path: &str, format: ManifestFormat, detail: String) -> Self {
        Self {
            path: path.to_string(),
            format,
            status: ManifestStatus::Unreadable,
            detail: Some(detail),
            artifacts: Vec::new(),
        }
    }
}

/// Every artifact fact one member's manifests yield, one [`ManifestFacts`] per
/// input, sorted by path.
///
/// `manifests` is **every** manifest the member holds, as `(project-relative
/// path, text)`: a pom's inherited group and properties come from the other poms
/// in the same slice, so a partial slice would refuse what a whole one resolves.
/// A path [`manifest_format`] does not name is skipped.
pub fn member_facts(manifests: &[(&str, &str)]) -> Vec<ManifestFacts> {
    let mut out = Vec::new();
    let mut poms: Vec<ParsedPom> = Vec::new();
    for &(path, text) in manifests {
        match manifest_format(path) {
            Some(ManifestFormat::Maven) => match parse_pom(text) {
                Ok(pom) => poms.push(ParsedPom {
                    path: path.to_string(),
                    pom,
                }),
                Err(detail) => out.push(ManifestFacts {
                    path: path.to_string(),
                    format: ManifestFormat::Maven,
                    status: ManifestStatus::Malformed,
                    detail: Some(detail),
                    artifacts: Vec::new(),
                }),
            },
            Some(ManifestFormat::Gradle) => out.push(ManifestFacts {
                path: path.to_string(),
                format: ManifestFormat::Gradle,
                status: ManifestStatus::Read,
                detail: None,
                artifacts: gradle_facts(text),
            }),
            None => {}
        }
    }

    let chain = Chain::new(&poms);
    for (index, pom) in poms.iter().enumerate() {
        out.push(ManifestFacts {
            path: pom.path.clone(),
            format: ManifestFormat::Maven,
            status: ManifestStatus::Read,
            detail: None,
            artifacts: chain.facts(index),
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

// ── Maven ────────────────────────────────────────────────────────────────────

/// A coordinate as declared, before interpolation.
#[derive(Debug, Default, Clone)]
struct RawCoord {
    group: Option<String>,
    artifact: Option<String>,
    version: Option<String>,
    scope: Option<String>,
    /// `<relativePath>` of a `<parent>`; `Some("")` for an explicit empty one.
    relative_path: Option<String>,
}

/// The parts of one pom this reader uses, as declared.
#[derive(Debug, Default)]
struct RawPom {
    group: Option<String>,
    artifact: Option<String>,
    version: Option<String>,
    parent: Option<RawCoord>,
    properties: BTreeMap<String, String>,
    dependencies: Vec<RawCoord>,
    managed: Vec<RawCoord>,
}

struct ParsedPom {
    path: String,
    pom: RawPom,
}

/// Set one coordinate field from an element's text. Empty text declares
/// nothing, except for `<relativePath/>`, whose emptiness is its meaning.
fn set_coord_field(coord: &mut RawCoord, field: &str, text: &str) {
    let slot = match field {
        "groupId" => &mut coord.group,
        "artifactId" => &mut coord.artifact,
        "version" => &mut coord.version,
        "scope" => &mut coord.scope,
        "relativePath" => {
            coord.relative_path = Some(text.to_string());
            return;
        }
        _ => return,
    };
    if !text.is_empty() {
        *slot = Some(text.to_string());
    }
}

/// Parse one pom's text, or describe why it is not a readable pom.
///
/// Streams with `quick-xml` and records only the exact element paths listed in
/// the module docs; everything else — plugins, exclusions, profiles, reporting —
/// is walked past by path. quick-xml expands no DTD entities, so an entity
/// declaration cannot inflate the input.
fn parse_pom(text: &str) -> Result<RawPom, String> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().check_end_names = true;

    let mut pom = RawPom::default();
    let mut stack: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut parent = RawCoord::default();
    let mut dependency = RawCoord::default();
    let mut saw_root = false;

    loop {
        let event = reader.read_event().map_err(|e| {
            format!(
                "malformed XML at byte {}: {e}",
                reader.error_position()
            )
        })?;
        match event {
            Event::Eof => break,
            Event::Start(e) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                open_element(&stack, &name, &mut saw_root)?;
                stack.push(name);
                buf.clear();
            }
            Event::Empty(e) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                open_element(&stack, &name, &mut saw_root)?;
                stack.push(name);
                close(&stack, "", &mut pom, &mut parent, &mut dependency);
                stack.pop();
                buf.clear();
            }
            Event::End(_) => {
                close(&stack, buf.trim(), &mut pom, &mut parent, &mut dependency);
                stack.pop();
                buf.clear();
            }
            Event::Text(t) => buf.push_str(
                &t.xml10_content()
                    .map_err(|e| format!("undecodable text: {e}"))?,
            ),
            Event::CData(c) => buf.push_str(
                &c.xml10_content()
                    .map_err(|e| format!("undecodable CDATA: {e}"))?,
            ),
            Event::GeneralRef(r) => match r.resolve_char_ref() {
                Ok(Some(ch)) => buf.push(ch),
                Ok(None) => {
                    let name = r.decode().map_err(|e| format!("undecodable entity: {e}"))?;
                    match name.as_ref() {
                        "lt" => buf.push('<'),
                        "gt" => buf.push('>'),
                        "amp" => buf.push('&'),
                        "apos" => buf.push('\''),
                        "quot" => buf.push('"'),
                        // An undeclared entity is kept as written: it is not a
                        // value this reader can know, and dropping it would
                        // silently shorten the text.
                        other => {
                            buf.push('&');
                            buf.push_str(other);
                            buf.push(';');
                        }
                    }
                }
                Err(e) => return Err(format!("invalid character reference: {e}")),
            },
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err("unclosed element(s) at end of document".to_string());
    }
    if !saw_root {
        return Err("no <project> root element".to_string());
    }
    Ok(pom)
}

/// Admit an element opening at `stack`: a root element must be the one and
/// only `<project>`.
fn open_element(stack: &[String], name: &str, saw_root: &mut bool) -> Result<(), String> {
    if !stack.is_empty() {
        return Ok(());
    }
    if *saw_root {
        return Err("more than one root element".to_string());
    }
    if name != "project" {
        return Err(format!("root element is <{name}>, not <project>"));
    }
    *saw_root = true;
    Ok(())
}

/// Record the element closing at the top of `stack`, whose text is `text`.
fn close(
    stack: &[String],
    text: &str,
    pom: &mut RawPom,
    parent: &mut RawCoord,
    dependency: &mut RawCoord,
) {
    let path: Vec<&str> = stack.iter().map(String::as_str).collect();
    match path.as_slice() {
        ["project", "groupId"] if !text.is_empty() => pom.group = Some(text.to_string()),
        ["project", "artifactId"] if !text.is_empty() => pom.artifact = Some(text.to_string()),
        ["project", "version"] if !text.is_empty() => pom.version = Some(text.to_string()),
        ["project", "parent", field] => set_coord_field(parent, field, text),
        ["project", "parent"] => pom.parent = Some(std::mem::take(parent)),
        ["project", "properties", name] => {
            pom.properties.insert((*name).to_string(), text.to_string());
        }
        ["project", "dependencies", "dependency", field]
        | ["project", "dependencyManagement", "dependencies", "dependency", field] => {
            set_coord_field(dependency, field, text);
        }
        ["project", "dependencies", "dependency"] => {
            pom.dependencies.push(std::mem::take(dependency));
        }
        ["project", "dependencyManagement", "dependencies", "dependency"] => {
            pom.managed.push(std::mem::take(dependency));
        }
        _ => {}
    }
}

/// The directory part of a project-relative path (`""` at the root).
fn parent_dir(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

/// Join a project-relative directory with a relative path, folding `.` and
/// `..`. `None` when the path climbs above the project root — the pom it names
/// is not in this member.
fn join_relative(dir: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// How deep one `${…}` may nest property references before it is refused.
/// Each level recurses once, so this is what keeps a crafted linear chain
/// (`<p0>${p1}</p0>`, `<p1>${p2}</p1>`, …) from exhausting the stack.
const MAX_EXPANSION_DEPTH: usize = 32;

/// How many property references one field may expand in total. A property
/// that references the next one twice doubles the work per level — the
/// `<properties>` form of an entity-expansion bomb — and this bounds it.
const MAX_SUBSTITUTIONS: usize = 256;

/// The longest value one field may expand to. A coordinate is a few dozen
/// bytes; anything past this is refused rather than built.
const MAX_EXPANDED_LEN: usize = 1024;

/// The state of one field's interpolation: the property names being expanded
/// (the cycle guard and the depth), and the references expanded so far.
#[derive(Default)]
struct Expansion {
    names: Vec<String>,
    substitutions: usize,
}

/// The in-member parent chain over one member's parsed poms.
struct Chain<'a> {
    poms: &'a [ParsedPom],
    /// Each pom's in-member parent, by index.
    parents: Vec<Option<usize>>,
}

impl<'a> Chain<'a> {
    fn new(poms: &'a [ParsedPom]) -> Self {
        let parents = (0..poms.len()).map(|i| link_parent(poms, i)).collect();
        Self { poms, parents }
    }

    /// Pom `index` and its in-member ancestors, nearest first, each once.
    fn lineage(&self, index: usize) -> Vec<usize> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        let mut cur = Some(index);
        while let Some(i) = cur {
            if !seen.insert(i) {
                break; // a parent cycle: every pom in it is visited once
            }
            out.push(i);
            cur = self.parents[i];
        }
        out
    }

    /// Interpolate every `${…}` in `raw` in the context of pom `index`.
    fn interpolate(&self, index: usize, raw: &str, active: &mut Expansion) -> Result<String, String> {
        let mut out = String::new();
        let mut rest = raw;
        while let Some(start) = rest.find("${") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let Some(end) = after.find('}') else {
                return Err(format!("`{raw}` has an unterminated `${{`"));
            };
            let name = &after[..end];
            if active.names.iter().any(|a| a == name) {
                return Err(format!("`${{{name}}}` is defined in terms of itself"));
            }
            if active.names.len() >= MAX_EXPANSION_DEPTH {
                return Err(format!(
                    "`${{{name}}}` nests property references deeper than {MAX_EXPANSION_DEPTH}"
                ));
            }
            active.substitutions += 1;
            if active.substitutions > MAX_SUBSTITUTIONS {
                return Err(format!(
                    "`{raw}` expands more than {MAX_SUBSTITUTIONS} property references"
                ));
            }
            active.names.push(name.to_string());
            let value = self.property(index, name, active);
            active.names.pop();
            out.push_str(&value?);
            if out.len() > MAX_EXPANDED_LEN {
                return Err(format!("`{raw}` expands beyond {MAX_EXPANDED_LEN} bytes"));
            }
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        if out.len() > MAX_EXPANDED_LEN {
            return Err(format!("`{raw}` expands beyond {MAX_EXPANDED_LEN} bytes"));
        }
        Ok(out)
    }

    /// The value of property `name` for pom `index`: a `project.*` model
    /// expression the pom answers, else the nearest `<properties>` definition in
    /// its in-member lineage — else refused, never defaulted.
    fn property(&self, index: usize, name: &str, active: &mut Expansion) -> Result<String, String> {
        let pom = &self.poms[index].pom;
        let parent = pom.parent.as_ref();
        let model = match name {
            "project.groupId" | "pom.groupId" => Some(self.effective_group(index, active)),
            "project.artifactId" | "pom.artifactId" => Some(match &pom.artifact {
                Some(a) => self.interpolate(index, a, active),
                None => Err("`${project.artifactId}`: the pom declares no <artifactId>".to_string()),
            }),
            "project.version" | "pom.version" => Some(match self.effective_version(index) {
                Some(v) => self.interpolate(index, v, active),
                None => Err(
                    "`${project.version}`: the pom declares no <version> and no <parent> to inherit one from"
                        .to_string(),
                ),
            }),
            "project.parent.groupId" => Some(self.parent_field(index, parent.and_then(|p| p.group.as_deref()), name, active)),
            "project.parent.artifactId" => Some(self.parent_field(index, parent.and_then(|p| p.artifact.as_deref()), name, active)),
            "project.parent.version" => Some(self.parent_field(index, parent.and_then(|p| p.version.as_deref()), name, active)),
            _ => None,
        };
        if let Some(value) = model {
            return value;
        }
        for ancestor in self.lineage(index) {
            if let Some(value) = self.poms[ancestor].pom.properties.get(name) {
                // Maven interpolates an inherited value in the child's context.
                return self.interpolate(index, value, active);
            }
        }
        Err(format!(
            "`${{{name}}}` is not defined in the pom's <properties> or its in-member parent chain{}",
            self.chain_boundary(index)
        ))
    }

    fn parent_field(
        &self,
        index: usize,
        value: Option<&str>,
        name: &str,
        active: &mut Expansion,
    ) -> Result<String, String> {
        match value {
            Some(v) => self.interpolate(index, v, active),
            None => Err(format!("`${{{name}}}`: the pom's <parent> does not declare it")),
        }
    }

    /// Where the in-member lineage of `index` stops, when it stops at a parent
    /// that is not a pom of this member — the place a missing property most
    /// likely lives, named so the refusal says where to look.
    fn chain_boundary(&self, index: usize) -> String {
        let top = *self.lineage(index).last().unwrap_or(&index);
        let Some(parent) = self.poms[top].pom.parent.as_ref() else {
            return String::new();
        };
        if self.parents[top].is_some() {
            return String::new(); // a cycle, not a boundary
        }
        format!(
            " (its parent `{}:{}` is not a pom of this member)",
            parent.group.as_deref().unwrap_or("?"),
            parent.artifact.as_deref().unwrap_or("?")
        )
    }

    /// The pom's group: its own, else the one its `<parent>` declares.
    fn effective_group(&self, index: usize, active: &mut Expansion) -> Result<String, String> {
        let pom = &self.poms[index].pom;
        match pom
            .group
            .as_deref()
            .or_else(|| pom.parent.as_ref().and_then(|p| p.group.as_deref()))
        {
            Some(g) => self.interpolate(index, g, active),
            None => Err("the pom declares no <groupId> and no <parent> to inherit one from".to_string()),
        }
    }

    /// The pom's declared version, else the one its `<parent>` declares.
    fn effective_version(&self, index: usize) -> Option<&str> {
        let pom = &self.poms[index].pom;
        pom.version
            .as_deref()
            .or_else(|| pom.parent.as_ref().and_then(|p| p.version.as_deref()))
    }

    /// Every fact pom `index` yields: produced first, then `<parent>`,
    /// `<dependencies>` and `<dependencyManagement>` in document order.
    fn facts(&self, index: usize) -> Vec<ArtifactFact> {
        let pom = &self.poms[index].pom;
        let mut out = Vec::new();

        let group = self.effective_group(index, &mut Expansion::default());
        out.push(self.fact(
            index,
            ArtifactRole::Produced,
            None,
            FieldState::from_result(group, pom.group.as_deref().or_else(|| {
                pom.parent.as_ref().and_then(|p| p.group.as_deref())
            })),
            pom.artifact.as_deref(),
            self.effective_version(index),
            None,
        ));

        if let Some(parent) = &pom.parent {
            out.push(self.reference(index, ReferenceKind::Parent, parent, false));
        }
        for dependency in &pom.dependencies {
            out.push(self.reference(index, ReferenceKind::Dependency, dependency, true));
        }
        for managed in &pom.managed {
            let kind = if managed.scope.as_deref() == Some("import") {
                ReferenceKind::BomImport
            } else {
                ReferenceKind::Managed
            };
            out.push(self.reference(index, kind, managed, true));
        }
        out
    }

    fn reference(&self, index: usize, kind: ReferenceKind, coord: &RawCoord, scoped: bool) -> ArtifactFact {
        let group = match &coord.group {
            Some(g) => FieldState::from_result(self.interpolate(index, g, &mut Expansion::default()), Some(g)),
            None => FieldState::Missing,
        };
        self.fact(
            index,
            ArtifactRole::Referenced,
            Some(kind),
            group,
            coord.artifact.as_deref(),
            coord.version.as_deref(),
            if scoped { coord.scope.clone() } else { None },
        )
    }

    /// Resolve one fact's artifact and version, and grade the outcome.
    #[allow(clippy::too_many_arguments)]
    fn fact(
        &self,
        index: usize,
        role: ArtifactRole,
        kind: Option<ReferenceKind>,
        group: FieldState,
        artifact: Option<&str>,
        version: Option<&str>,
        scope: Option<String>,
    ) -> ArtifactFact {
        let artifact = match artifact {
            Some(a) => FieldState::from_result(self.interpolate(index, a, &mut Expansion::default()), Some(a)),
            None => FieldState::Missing,
        };
        let version = version.map(|v| (v, self.interpolate(index, v, &mut Expansion::default())));

        let mut reasons = Vec::new();
        let group_id = group.into_value("groupId", &mut reasons);
        let artifact_id = artifact.into_value("artifactId", &mut reasons);
        let key_refused = !reasons.is_empty();
        let version = match version {
            None => None,
            Some((_, Ok(v))) => Some(v),
            Some((raw, Err(why))) => {
                reasons.push(format!("version: {why}"));
                Some(raw.to_string())
            }
        };
        let resolution = if key_refused {
            Resolution::Refused
        } else if reasons.is_empty() {
            Resolution::Resolved
        } else {
            Resolution::VersionRefused
        };
        ArtifactFact {
            role,
            kind,
            group_id,
            artifact_id,
            version,
            scope,
            project_path: None,
            resolution,
            reason: (!reasons.is_empty()).then(|| reasons.join("; ")),
        }
    }
}

/// One key field of a fact on its way to being graded.
enum FieldState {
    Resolved(String),
    Refused { declared: String, why: String },
    Missing,
}

impl FieldState {
    fn from_result(result: Result<String, String>, declared: Option<&str>) -> Self {
        match (result, declared) {
            (Ok(v), _) => FieldState::Resolved(v),
            (Err(why), Some(d)) => FieldState::Refused {
                declared: d.to_string(),
                why,
            },
            // A refusal with no declared text (a pom with neither a group nor a
            // parent) is stored as absent, never as an empty string.
            (Err(why), None) => FieldState::Refused {
                declared: String::new(),
                why,
            },
        }
    }

    fn into_value(self, field: &str, reasons: &mut Vec<String>) -> Option<String> {
        match self {
            FieldState::Resolved(v) => Some(v),
            FieldState::Refused { declared, why } => {
                reasons.push(format!("{field}: {why}"));
                (!declared.is_empty()).then_some(declared)
            }
            FieldState::Missing => {
                reasons.push(format!("{field}: not declared"));
                None
            }
        }
    }
}

/// The in-member pom `index`'s `<parent>` names, if exactly one does.
///
/// A candidate is a pom whose own coordinates — its `artifactId` and its
/// `groupId` or, lacking one, its parent's — equal the parent reference's
/// literally. One candidate links. Several are told apart by `<relativePath>`
/// (default `../pom.xml`); if that does not single one out, nothing links, and
/// every property the chain would have supplied is refused rather than taken
/// from an arbitrary one of them.
fn link_parent(poms: &[ParsedPom], index: usize) -> Option<usize> {
    let parent = poms[index].pom.parent.as_ref()?;
    let (group, artifact) = (parent.group.as_deref()?, parent.artifact.as_deref()?);
    let candidates: Vec<usize> = (0..poms.len())
        .filter(|&j| j != index)
        .filter(|&j| {
            let pom = &poms[j].pom;
            let own_group = pom
                .group
                .as_deref()
                .or_else(|| pom.parent.as_ref().and_then(|p| p.group.as_deref()));
            own_group == Some(group) && pom.artifact.as_deref() == Some(artifact)
        })
        .collect();
    match candidates.as_slice() {
        [] => None,
        [only] => Some(*only),
        several => {
            let rel = parent.relative_path.as_deref().unwrap_or("../pom.xml");
            let rel = if rel.ends_with(".xml") {
                rel.to_string()
            } else {
                format!("{}/pom.xml", rel.trim_end_matches('/'))
            };
            let expected = join_relative(parent_dir(&poms[index].path), &rel)?;
            let mut at = several.iter().filter(|&&j| poms[j].path == expected);
            match (at.next(), at.next()) {
                (Some(&j), None) => Some(j),
                _ => None,
            }
        }
    }
}

// ── Gradle ───────────────────────────────────────────────────────────────────

/// The reason every Gradle produced fact is refused.
const GRADLE_NAME_UNREAD: &str = "artifactId: a Gradle project's artifact name comes from \
     settings.gradle or its directory, neither of which is read";

/// Read a Gradle build script's facts: one produced fact carrying the project
/// `group`, then its literal dependency declarations in order.
///
/// A line-and-brace reader, not a Groovy/Kotlin parser. It tracks `{ }` blocks
/// outside strings and comments, reads a declaration only when the innermost
/// open block is `dependencies`, and reads `group` / `version` only at the top
/// level or inside `allprojects`. Map notation (`group: 'g', name: 'a'`),
/// version catalogs (`libs.x`) and `files(…)` are not literal coordinates and
/// are skipped.
fn gradle_facts(text: &str) -> Vec<ArtifactFact> {
    let mut group: Option<String> = None;
    let mut version: Option<String> = None;
    let mut references = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut in_block_comment = false;

    for line in text.lines() {
        let code = strip_comments(line, &mut in_block_comment);
        let mut segment = String::new();
        let mut quote: Option<char> = None;
        let mut chars = code.chars().peekable();
        while let Some(ch) = chars.next() {
            if let Some(q) = quote {
                segment.push(ch);
                if ch == '\\' {
                    if let Some(next) = chars.next() {
                        segment.push(next);
                    }
                } else if ch == q {
                    quote = None;
                }
                continue;
            }
            match ch {
                '\'' | '"' => {
                    quote = Some(ch);
                    segment.push(ch);
                }
                '{' => {
                    gradle_statement(&segment, &stack, &mut group, &mut version, &mut references);
                    stack.push(block_name(&segment));
                    segment.clear();
                }
                '}' => {
                    gradle_statement(&segment, &stack, &mut group, &mut version, &mut references);
                    stack.pop();
                    segment.clear();
                }
                _ => segment.push(ch),
            }
        }
        gradle_statement(&segment, &stack, &mut group, &mut version, &mut references);
    }

    let mut reasons = Vec::new();
    let group_id = match group {
        Some(g) if g.contains('$') => {
            reasons.push(format!("groupId: `{g}` interpolates a value that is not read"));
            Some(g)
        }
        Some(g) => Some(g),
        None => {
            reasons.push("groupId: the script declares no literal `group`".to_string());
            None
        }
    };
    reasons.push(GRADLE_NAME_UNREAD.to_string());
    let mut out = vec![ArtifactFact {
        role: ArtifactRole::Produced,
        kind: None,
        group_id,
        artifact_id: None,
        version,
        scope: None,
        project_path: None,
        resolution: Resolution::Refused,
        reason: Some(reasons.join("; ")),
    }];
    out.extend(references);
    out
}

/// The identifier immediately before an opening brace (`dependencies {` →
/// `dependencies`), or `""` when there is none (`) {`).
fn block_name(segment: &str) -> String {
    let trimmed = segment.trim_end();
    // Walk back over identifier characters by char, not byte: the character
    // before the name may be multi-byte (`—{`), and a byte offset past it would
    // slice inside it.
    let start = trimmed
        .char_indices()
        .rev()
        .take_while(|&(_, c)| c.is_alphanumeric() || c == '_')
        .last()
        .map_or(trimmed.len(), |(i, _)| i);
    trimmed[start..].to_string()
}

/// Remove `//` and `/* */` comments from one line, outside string literals.
fn strip_comments(line: &str, in_block: &mut bool) -> String {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    let bytes: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i];
        let next = bytes.get(i + 1).copied();
        if *in_block {
            if ch == '*' && next == Some('/') {
                *in_block = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if let Some(q) = quote {
            out.push(ch);
            if ch == '\\' {
                if let Some(n) = next {
                    out.push(n);
                    i += 1;
                }
            } else if ch == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match (ch, next) {
            ('/', Some('/')) => break,
            ('/', Some('*')) => {
                *in_block = true;
                i += 2;
            }
            ('\'' | '"', _) => {
                quote = Some(ch);
                out.push(ch);
                i += 1;
            }
            _ => {
                out.push(ch);
                i += 1;
            }
        }
    }
    out
}

/// Interpret one brace-free statement of a Gradle script.
fn gradle_statement(
    segment: &str,
    stack: &[String],
    group: &mut Option<String>,
    version: &mut Option<String>,
    references: &mut Vec<ArtifactFact>,
) {
    let statement = segment.trim();
    if statement.is_empty() {
        return;
    }
    match stack.last().map(String::as_str) {
        Some("dependencies") => {
            if let Some(fact) = gradle_dependency(statement) {
                references.push(fact);
            }
        }
        None | Some("allprojects") => {
            if group.is_none() {
                *group = gradle_assignment(statement, "group");
            }
            if version.is_none() {
                *version = gradle_assignment(statement, "version");
            }
        }
        Some(_) => {}
    }
}

/// `key = 'v'`, `key = "v"` or `key 'v'` → `v`; anything else → `None`.
fn gradle_assignment(statement: &str, key: &str) -> Option<String> {
    let rest = statement.strip_prefix(key)?;
    if rest.starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '.') {
        return None; // `groupId = …`, `group.foo = …`
    }
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim_start();
    let (value, tail) = quoted(rest)?;
    tail.trim().is_empty().then_some(value)
}

/// A leading quoted literal: its content and what follows the closing quote.
fn quoted(s: &str) -> Option<(String, &str)> {
    let mut chars = s.char_indices();
    let (_, q) = chars.next()?;
    if q != '\'' && q != '"' {
        return None;
    }
    let mut value = String::new();
    let mut escaped = false;
    for (i, ch) in chars {
        if escaped {
            value.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == q {
            return Some((value, &s[i + ch.len_utf8()..]));
        } else {
            value.push(ch);
        }
    }
    None
}

/// Read one declaration inside `dependencies { }`: `<configuration>` followed by
/// a literal coordinate, a `project(':x')`, or a `platform(…)` /
/// `enforcedPlatform(…)` of a literal coordinate — with or without parentheses.
fn gradle_dependency(statement: &str) -> Option<ArtifactFact> {
    let end = statement
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(statement.len());
    let configuration = &statement[..end];
    if configuration.is_empty() || configuration.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let mut rest = statement[end..].trim_start();
    if let Some(inner) = rest.strip_prefix('(') {
        rest = inner.trim_start();
    }
    let scope = Some(configuration.to_string());

    if let Some(args) = rest.strip_prefix("project(") {
        let args = args.trim_start();
        let args = args
            .strip_prefix("path")
            .map(|a| a.trim_start().trim_start_matches([':', '=']).trim_start())
            .unwrap_or(args);
        let (path, _) = quoted(args)?;
        return Some(ArtifactFact {
            role: ArtifactRole::Referenced,
            kind: Some(ReferenceKind::Dependency),
            group_id: None,
            artifact_id: None,
            version: None,
            scope,
            project_path: Some(path),
            resolution: Resolution::Resolved,
            reason: None,
        });
    }
    let (kind, rest) = ["enforcedPlatform(", "platform("]
        .iter()
        .find_map(|p| rest.strip_prefix(p))
        .map_or((ReferenceKind::Dependency, rest), |inner| {
            (ReferenceKind::BomImport, inner.trim_start())
        });
    let (literal, _) = quoted(rest)?;
    gradle_coordinate(&literal, kind, scope)
}

/// `group:artifact[:version[:classifier]][@ext]` → a fact, or `None` when the
/// literal is not a coordinate at all.
fn gradle_coordinate(literal: &str, kind: ReferenceKind, scope: Option<String>) -> Option<ArtifactFact> {
    let literal = literal.split('@').next().unwrap_or(literal);
    let parts: Vec<&str> = literal.split(':').collect();
    // A coordinate part never holds a slash or whitespace, so a URL or a path
    // with colons in it (`'http://host/a:b'`) is not misread as one.
    if !(2..=4).contains(&parts.len())
        || parts[..2].iter().any(|p| p.is_empty())
        || parts.iter().any(|p| p.contains(|c: char| c == '/' || c.is_whitespace()))
    {
        return None;
    }
    let (group, artifact) = (parts[0], parts[1]);
    let version = parts.get(2).filter(|v| !v.is_empty()).copied();
    let mut reasons = Vec::new();
    for (field, value) in [("groupId", group), ("artifactId", artifact)] {
        if value.contains('$') {
            reasons.push(format!("{field}: `{value}` interpolates a value that is not read"));
        }
    }
    let key_refused = !reasons.is_empty();
    if let Some(v) = version.filter(|v| v.contains('$')) {
        reasons.push(format!("version: `{v}` interpolates a value that is not read"));
    }
    let resolution = if key_refused {
        Resolution::Refused
    } else if reasons.is_empty() {
        Resolution::Resolved
    } else {
        Resolution::VersionRefused
    };
    Some(ArtifactFact {
        role: ArtifactRole::Referenced,
        kind: Some(kind),
        group_id: Some(group.to_string()),
        artifact_id: Some(artifact.to_string()),
        version: version.map(str::to_string),
        scope,
        project_path: None,
        resolution,
        reason: (!reasons.is_empty()).then(|| reasons.join("; ")),
    })
}

#[cfg(test)]
#[path = "build_manifest_tests.rs"]
mod tests;
