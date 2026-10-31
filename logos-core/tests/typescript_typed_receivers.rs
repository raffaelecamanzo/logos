//! A TypeScript call on a receiver of proven declared type binds among that
//! type's members (S-537, CR-173, FR-RS-21, FR-RS-10, FR-RS-12, NFR-RA-05) —
//! exercised end to end through the public [`Engine`] façade against
//! temp-directory fixtures, with the SHIPPED `references.scm` and
//! `plugin.toml` of the TypeScript plugin (no query override).
//!
//! What the fixtures pin, over a NestJS-shaped service calling the classes it
//! is handed:
//!
//! - a receiver whose type the file proves — a constructor parameter property
//!   (`constructor(private readonly users: UsersService)`, the dominant
//!   injection shape), a field annotation, a parameter, a typed local and
//!   `const x = new T()` — is the Path-form `T::m` of shape `other`, `T`
//!   qualified by the named import it came through, and binds among `T`'s
//!   members: a class's own, a base class's through the proven hierarchy, and
//!   an abstract class's or an interface's declared — bodyless — method;
//! - a receiver the file proves no type for — an untyped or inferred one, an
//!   arrow function's parameter, a field read without `this` — stays the
//!   Method-form row of shape `other`, which the binder binds nowhere
//!   (`no-receiver-evidence`);
//! - a type the repository does not declare binds nothing: the proven row
//!   stays unbound (`external-type`).
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite (`csharp_typed_receivers.rs`): a fixture tree checked into this
//! repository would be indexed into its own graph.
//!
//! [FR-RS-21]: ../../docs/specs/requirements/FR-RS-21.md

#![cfg(feature = "lang-typescript")]

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId, ReceiverShape, RefForm};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

#[path = "support/coordinate.rs"]
mod coordinate;

fn tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in files {
        let path = tmp.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    coordinate::pin(tmp.path());
    tmp
}

fn index(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// A node's label: `file:name@line`.
fn labels(rt: &Runtime) -> HashMap<NodeId, String> {
    rt.submit_read(|store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .map(|n| {
                let file = n.file_path.unwrap_or_default();
                (n.id, format!("{file}:{}@{}", n.name, n.start_line.unwrap_or(0)))
            })
            .collect())
    })
    .expect("read runs")
}

/// Bound edges as `(source label, target label)`.
type Edges = Vec<(String, String)>;

/// Every bound `Calls` edge as `(source label, target label)`, sorted.
fn calls(rt: &Runtime) -> Edges {
    let label = labels(rt);
    let mut out: Edges = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .map(|e| (label[&e.source].clone(), label[&e.target].clone()))
        .collect();
    out.sort();
    out
}

/// One ledger row: `(source label, target, form, receiver, resolved)`.
type Row = (String, String, RefForm, Option<ReceiverShape>, bool);

/// The `Calls` rows sourced in `file` whose target ends in one of `names`,
/// sorted.
fn rows(rt: &Runtime, file: &str, names: &[&str]) -> Vec<Row> {
    let label = labels(rt);
    let by_symbol: HashMap<String, String> = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .map(|n| (n.symbol.as_str().to_string(), label[&n.id].clone()))
                .collect())
        })
        .expect("read runs");
    let prefix = format!("{file}:");
    let mut rows: Vec<Row> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Calls && r.form != RefForm::Symbol)
        .filter(|r| names.iter().any(|n| r.target == *n || r.target.ends_with(&format!("::{n}"))))
        .filter_map(|r| {
            let label = by_symbol.get(&r.source_symbol)?;
            label
                .starts_with(&prefix)
                .then(|| (label.clone(), r.target, r.form, r.receiver, r.resolved))
        })
        .collect();
    rows.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    rows
}

fn edge(from: &str, to: &str) -> (String, String) {
    (from.to_string(), to.to_string())
}

const OTHER: Option<ReceiverShape> = Some(ReceiverShape::Other);

const USERS: &str = "\
export class UsersService {
  findByEmail(email: string) { return email; }
  findById(id: string) { return id; }
}
export class SpecialService extends UsersService {}
";

const REPOSITORY: &str = "\
export abstract class UserRepository {
  abstract findById(id: string): string;
  count(): number { return 0; }
}
export interface Notifier {
  notify(message: string): void;
}
";

const AUTH: &str = "\
import { UsersService, SpecialService } from '../users/users.service';
import { UserRepository, Notifier } from '../users/user.repository';
import { Logger } from '@nestjs/common';

class Mailer { send(to: string) { return to; } }

export class AuthService {
  private mailer: Mailer;
  private readonly logger: Logger;
  constructor(
    private readonly usersService: UsersService,
    private repo: UserRepository,
    private readonly notifier: Notifier,
  ) {}
  login(email: string) { return this.usersService.findByEmail(email); }
  byRepo(id: string) { return this.repo.findById(id); }
  byRepoBody() { return this.repo.count(); }
  byInterface() { this.notifier.notify('x'); }
  byField() { return this.mailer.send('a'); }
  byExternal() { this.logger.log('x'); }
  byParam(svc: UsersService) { return svc.findById('1'); }
  byLocal() { const s: UsersService = make(); return s.findById('2'); }
  byNew() { const made = new UsersService(); return made.findByEmail('3'); }
  bySuper(special: SpecialService) { return special.findById('4'); }
  untyped(x) { return x.findById('5'); }
  inferred() { const got = make(); return got.findById('6'); }
  lambda(xs: UsersService[]) { return xs.map(each => each.findById('7')); }
}
function make(): any { return null; }
";

const NAMES: &[&str] = &["findByEmail", "findById", "count", "notify", "send", "log"];

#[test]
fn a_call_on_a_receiver_of_proven_type_binds_among_that_types_members() {
    let tmp = tree(&[
        ("src/users/users.service.ts", USERS),
        ("src/users/user.repository.ts", REPOSITORY),
        ("src/auth/auth.service.ts", AUTH),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let a = |line: u32, name: &str| format!("src/auth/auth.service.ts:{name}@{line}");
    let users = "..::users::users.service";
    let repository = "..::users::user.repository";
    let proven = |source: String, target: String, resolved: bool| (source, target, RefForm::Path, OTHER, resolved);
    let untyped = |source: String, name: &str| (source, name.to_string(), RefForm::Method, OTHER, false);
    assert_eq!(
        rows(rt, "src/auth/auth.service.ts", NAMES),
        vec![
            proven(a(20, "byExternal"), "@nestjs::common::Logger::log".to_string(), false),
            proven(a(19, "byField"), "Mailer::send".to_string(), true),
            proven(a(18, "byInterface"), format!("{repository}::Notifier::notify"), true),
            proven(a(22, "byLocal"), format!("{users}::UsersService::findById"), true),
            proven(a(23, "byNew"), format!("{users}::UsersService::findByEmail"), true),
            proven(a(21, "byParam"), format!("{users}::UsersService::findById"), true),
            proven(a(16, "byRepo"), format!("{repository}::UserRepository::findById"), true),
            proven(a(17, "byRepoBody"), format!("{repository}::UserRepository::count"), true),
            proven(a(24, "bySuper"), format!("{users}::SpecialService::findById"), true),
            untyped(a(26, "inferred"), "findById"),
            untyped(a(27, "lambda"), "findById"),
            proven(a(15, "login"), format!("{users}::UsersService::findByEmail"), true),
            untyped(a(25, "untyped"), "findById"),
        ]
    );
    let edges = calls(rt);
    let u = |line: u32, name: &str| format!("src/users/users.service.ts:{name}@{line}");
    let r = |line: u32, name: &str| format!("src/users/user.repository.ts:{name}@{line}");
    for expected in [
        edge(&a(15, "login"), &u(2, "findByEmail")),
        // An abstract class's bodyless method, and an interface's: the
        // declared method, never an implementation.
        edge(&a(16, "byRepo"), &r(2, "findById")),
        edge(&a(17, "byRepoBody"), &r(3, "count")),
        edge(&a(18, "byInterface"), &r(6, "notify")),
        edge(&a(19, "byField"), "src/auth/auth.service.ts:send@5"),
        edge(&a(21, "byParam"), &u(3, "findById")),
        edge(&a(22, "byLocal"), &u(3, "findById")),
        edge(&a(23, "byNew"), &u(2, "findByEmail")),
        // Inheritance: `SpecialService` declares no `findById`; its base does.
        edge(&a(24, "bySuper"), &u(3, "findById")),
    ] {
        assert!(edges.contains(&expected), "missing {expected:?} in {edges:#?}");
    }
    for caller in ["untyped", "inferred", "lambda", "byExternal"] {
        assert!(
            !edges.iter().any(|(from, to)| from.contains(&format!(":{caller}@")) && !to.contains(":make@")),
            "{caller} proves no in-repository type: {edges:#?}"
        );
    }
}

// ── What the file does not prove ────────────────────────────────────────────

/// Every call below names `findById` on a receiver the file proves no type
/// for, though a plain reading would hand it one: a same-named binding with
/// a proof sits elsewhere in the file, in the callable, or in another class.
const UNPROVEN: &str = "\
import { UsersService } from '../users/users.service';
import * as ns from '../users/users.service';

const svc = pick();

export class Holder {
  private svc: UsersService;
  bare() { return svc.findById('1'); }
  shadowed(each: UsersService) { return [1].map(each => each.findById('2')); }
  destructured({ users }: { users: UsersService }) { return users.findById('3'); }
  qualified(q: ns.UsersService) { return q.findById('4'); }
  caught() { try { pick(); } catch (err) { err.findById('5'); } }
  looped(all: UsersService[]) { for (const one of all) { one.findById('6'); } }
  chained() { return this.svc.self().findById('7'); }
  private q: ns.UsersService;
  viaThis() { return this.q.findById('8'); }
  nested(s: UsersService, items: any[]) { items.forEach(item => { const s = item.owner; s.findById('9'); }); }
  block(s: UsersService) { { const s = pick(); s.findById('10'); } }
  inner(s: UsersService) { function deeper() { let s = pick(); return s.findById('11'); } return deeper; }
  moduleLevel() { const f = (svc: UsersService) => svc; return svc.findById('12'); }
  union(u: UsersService | null) { const f = (u: UsersService) => u; return u.findById('13'); }
  anyTyped(a: any) { const f = (a: UsersService) => a; return a.findById('14'); }
  private g: Array<UsersService> | null;
  generic() { return this.g.findById('15'); }
}
export class Injected { constructor(private readonly other: UsersService) {} }
interface Deps { other: UsersService; plain: UsersService; }
export class Untyped {
  private other = pick();
  plain = pick();
  viaOther() { return this.other.findById('16'); }
  viaPlain() { return this.plain.findById('17'); }
}
class Fmt { findById() { return 0; } }
export function onClass() { return Fmt.findById('18'); }
export class Statics {
  static st: UsersService;
  st = pick();
  viaInstance() { return this.st.findById('19'); }
}
function pick(): any { return null; }
";

#[test]
fn a_receiver_the_file_proves_no_type_for_binds_nowhere() {
    let tmp = tree(&[("src/users/users.service.ts", USERS), ("src/holder/holder.ts", UNPROVEN)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let receiver_calls = rows(rt, "src/holder/holder.ts", &["findById"]);
    assert_eq!(receiver_calls.len(), 19, "{receiver_calls:#?}");
    let typed: Vec<&Row> = receiver_calls
        .iter()
        .filter(|r| (&r.1[..], r.2, r.3, r.4) != ("findById", RefForm::Method, OTHER, false))
        .collect();
    assert!(typed.is_empty(), "every receiver here must stay untyped: {typed:#?}");
    let edges = calls(rt);
    assert!(
        !edges.iter().any(|(_, to)| to.starts_with("src/users/") || to.contains(":findById@")),
        "nothing here proves a type: {edges:#?}"
    );
}

// ── The TSX grammar and a `#private` field ─────────────────────────────────

const WIDGET: &str = "\
import { UsersService } from '../users/users.service';

export class Widget {
  #users: UsersService;
  constructor(private readonly svc: UsersService) {}
  render() { const made = new UsersService(); return this.#users.findById(made.findByEmail('1')); }
  viaProperty() { return this.svc.findById('2'); }
  shadow(s: UsersService) { return [1].map(s => s.findById('3')); }
}
";

#[test]
fn a_tsx_file_types_its_receivers_alike() {
    let tmp = tree(&[("src/users/users.service.ts", USERS), ("src/ui/widget.tsx", WIDGET)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let w = |line: u32, name: &str| format!("src/ui/widget.tsx:{name}@{line}");
    let from_widget: Edges = calls(rt).into_iter().filter(|(from, _)| from.starts_with("src/ui/")).collect();
    assert_eq!(
        from_widget,
        vec![
            edge(&w(6, "render"), "src/users/users.service.ts:findByEmail@2"),
            edge(&w(6, "render"), "src/users/users.service.ts:findById@3"),
            edge(&w(7, "viaProperty"), "src/users/users.service.ts:findById@3"),
        ]
    );
}

// ── Every declaration and binding form ─────────────────────────────────────

const REPO: &str = "\
export class Repo<T> { find() { return null; } }
";

/// Each declaration form TypeScript writes that proves a type.
const FORMS: &str = "\
import { UsersService, SpecialService } from '../users/users.service';
import { Repo } from '../users/repo';

export class Forms {
  #own: UsersService;
  private repo: Repo<string>;
  constructor(private readonly opt?: UsersService) {}
  byPrivate() { return this.#own.findById('1'); }
  byGenericField() { return this.repo.find(); }
  byOptionalProperty() { return this.opt.findById('2'); }
  byGenericParam(r: Repo<number>) { return r.find(); }
  byOptional(svc?: UsersService) { return svc.findById('3'); }
  byWidened() { const x: UsersService = new SpecialService(); return x.findById('4'); }
  byVar() { var v: UsersService = pick(); return v.findById('5'); }
}
function pick(): any { return null; }
";

#[test]
fn every_declaration_form_typescript_writes_proves_its_type() {
    let tmp = tree(&[
        ("src/users/users.service.ts", USERS),
        ("src/users/repo.ts", REPO),
        ("src/f/forms.ts", FORMS),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let f = |line: u32, name: &str| format!("src/f/forms.ts:{name}@{line}");
    let (find_by_id, find) = ("src/users/users.service.ts:findById@3", "src/users/repo.ts:find@1");
    let from_forms: Edges = calls(rt).into_iter().filter(|(from, _)| from.starts_with("src/f/")).collect();
    assert_eq!(
        from_forms,
        vec![
            edge(&f(9, "byGenericField"), find),
            edge(&f(11, "byGenericParam"), find),
            edge(&f(12, "byOptional"), find_by_id),
            edge(&f(10, "byOptionalProperty"), find_by_id),
            edge(&f(8, "byPrivate"), find_by_id),
            edge(&f(14, "byVar"), "src/f/forms.ts:pick@16"),
            edge(&f(14, "byVar"), find_by_id),
            edge(&f(13, "byWidened"), find_by_id),
        ]
    );
    // The annotation is the proof, not the value it is given.
    let widened = rows(rt, "src/f/forms.ts", &["findById"]).into_iter().find(|r| r.0 == f(13, "byWidened"));
    assert_eq!(widened.map(|r| r.1), Some("..::users::users.service::UsersService::findById".to_string()));
}

/// A module-level binding with a proof, hidden in each callable by a binding
/// of the same name that has none.
const HIDDEN: &str = "\
import { UsersService } from '../users/users.service';

const top: UsersService = pick();
top.findById('module');
export function fd(top) { return top.findById('fd'); }
export const fe = function (top) { return top.findById('fe'); };
export function gd() { function* g(top) { yield top.findById('gd'); } return g; }
export function ge() { const g = function* (top) { yield top.findById('ge'); }; return g; }
export const ap = (top) => top.findById('ap');
export const as1 = top => top.findById('as');
export const de = ({ top }) => top.findById('de');
export class M { m(top) { return top.findById('m'); } }
export function ca() { try { pick(); } catch (top) { return top.findById('ca'); } }
export function fo(xs: any[]) { for (const top of xs) { top.findById('fo'); } }
export function va() { if (pick()) { var top = pick(); } return top.findById('va'); }
export function le() { { let top = pick(); return top.findById('le'); } }
function pick(): any { return null; }
";

#[test]
fn every_binding_form_hides_a_proof_from_outside_its_scope() {
    let tmp = tree(&[("src/users/users.service.ts", USERS), ("src/h/hidden.ts", HIDDEN)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let rows = rows(rt, "src/h/hidden.ts", &["findById"]);
    let (typed, untyped): (Vec<Row>, Vec<Row>) = rows.into_iter().partition(|r| r.2 == RefForm::Path);
    // Only the module-level call sees the module-level binding alone.
    assert_eq!(typed.iter().map(|r| (&r.0[..], r.4)).collect::<Vec<_>>(), vec![("src/h/hidden.ts:hidden@1", true)]);
    // One row per callable: a generator is no symbol, so each sits in a
    // function of its own.
    assert_eq!(untyped.len(), 12, "{untyped:#?}");
    assert!(untyped.iter().all(|r| (r.2, r.3, r.4) == (RefForm::Method, OTHER, false)), "{untyped:#?}");
}

// ── Each binding is read in its own scope ──────────────────────────────────

/// One name, a different proven type in each callable, and a module-level
/// binding read where no other is in scope.
const SCOPED: &str = "\
import { UsersService } from '../users/users.service';
import { UserRepository } from '../users/user.repository';

const made = new UsersService();
made.findByEmail('top');

export class Scoped {
  a(s: UsersService) { return s.findById('a'); }
  b(s: UserRepository) { return s.findById('b'); }
  c() { const s = new UsersService(); return s.findByEmail('c'); }
  d(xs: string[]) { return xs.map((s: UserRepository) => s.count()); }
}
export class Owner { constructor(private readonly r: UserRepository) {} e() { return this.r.count(); } }
export class Stranger { r = Math.random(); }
";

#[test]
fn each_binding_is_read_in_its_own_scope() {
    let tmp = tree(&[
        ("src/users/users.service.ts", USERS),
        ("src/users/user.repository.ts", REPOSITORY),
        ("src/s/scoped.ts", SCOPED),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let mut from_scoped: Edges = calls(rt).into_iter().filter(|(from, _)| from.starts_with("src/s/")).collect();
    from_scoped.sort();
    assert_eq!(
        from_scoped,
        vec![
            edge("src/s/scoped.ts:a@8", "src/users/users.service.ts:findById@3"),
            edge("src/s/scoped.ts:b@9", "src/users/user.repository.ts:findById@2"),
            edge("src/s/scoped.ts:c@10", "src/users/users.service.ts:findByEmail@2"),
            edge("src/s/scoped.ts:d@11", "src/users/user.repository.ts:count@3"),
            // Another class's untyped `r` is not this class's field.
            edge("src/s/scoped.ts:e@13", "src/users/user.repository.ts:count@3"),
            edge("src/s/scoped.ts:scoped@1", "src/users/users.service.ts:findByEmail@2"),
        ]
    );
}

// ── Sync ≡ reindex ───────────────────────────────────────────────────────────

/// Index `initial`, overwrite `edits` and sync them; then index the post-edit
/// tree from scratch. The two graphs must be identical (NFR-RA-06); the synced
/// one's `Calls` edges are returned.
fn synced_equals_reindexed(initial: &[(&str, &str)], edits: &[(&str, &str)]) -> Edges {
    let tmp_a = tree(initial);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    let mut changed: Vec<PathBuf> = Vec::new();
    for (rel, text) in edits {
        let path = tmp_a.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
        changed.push(root_a.join(rel));
    }
    engine_a.sync(&changed);
    let rt_a = engine_a.runtime().unwrap();

    let mut final_state: BTreeMap<&str, &str> = initial.iter().copied().collect();
    final_state.extend(edits.iter().copied());
    let files: Vec<(&str, &str)> = final_state.into_iter().collect();
    let tmp_b = tree(&files);
    let engine_b = index(tmp_b.path());

    assert_eq!(
        graph_fingerprint(rt_a),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    calls(rt_a)
}

const CALLER: &str = "\
import { UserRepository } from './user.repository';

export class Caller {
  constructor(private repo: UserRepository) {}
  run() { return this.repo.findById('1'); }
}
";

#[test]
fn a_synced_edit_to_the_receivers_type_matches_a_fresh_reindex() {
    let initial = [("src/users/user.repository.ts", REPOSITORY), ("src/users/caller.ts", CALLER)];
    let run = "src/users/caller.ts:run@5";
    // The abstract method is renamed away: the edge goes, though the caller's
    // file never changes.
    let renamed = REPOSITORY.replace("abstract findById", "abstract findOne");
    let edges = synced_equals_reindexed(&initial, &[("src/users/user.repository.ts", &renamed)]);
    assert!(edges.is_empty(), "{edges:?}");

    // The type arrives after the caller: the call binds on sync.
    let edges = synced_equals_reindexed(
        &[("src/users/caller.ts", CALLER)],
        &[("src/users/user.repository.ts", REPOSITORY)],
    );
    assert_eq!(edges, vec![edge(run, "src/users/user.repository.ts:findById@2")]);

    // A base class gains the method its subclass is typed by.
    let base = "export class Base {}\nexport class UserRepository extends Base {}\n";
    let gained = "export class Base { findById(id: string) { return id; } }\nexport class UserRepository extends Base {}\n";
    let edges = synced_equals_reindexed(
        &[("src/users/user.repository.ts", base), ("src/users/caller.ts", CALLER)],
        &[("src/users/user.repository.ts", gained)],
    );
    assert_eq!(edges, vec![edge(run, "src/users/user.repository.ts:findById@1")]);
}

