//! The workspace-wide live read-connection budget ([NFR-PE-11], [ADR-63]).
//!
//! [ADR-02] sizes an [`Engine`](crate::Engine)'s read pool to the host core
//! count, which is right under its premise of **one engine per process**.
//! Federation breaks that premise: [FR-WS-03](../../../docs/specs/requirements/FR-WS-03.md)
//! holds one engine per member, so an unbounded resident set costs
//! `members × cores` live connections. Measured on a 12-core host, that
//! exhausted the stock 256-descriptor macOS soft limit at member 10 of 72.
//!
//! A [`ConnectionBudget`] replaces that per-member multiple with a **host-derived
//! ceiling**. It answers three questions the [registry](super::EngineRegistry)
//! asks: how many read connections one resident member may open
//! ([`per_member_read_connections`](ConnectionBudget::per_member_read_connections)),
//! how many members may be resident at once
//! ([`max_resident_members`](ConnectionBudget::max_resident_members)), and how
//! many `rayon` workers the whole resident set shares
//! ([`worker_threads`](ConnectionBudget::worker_threads)). The first two multiply
//! into the ceiling on live read connections
//! ([`total_read_connections`](ConnectionBudget::total_read_connections)); the
//! third is the thread ceiling — together, the two quantities [NFR-PE-11] bounds.
//!
//! Because the connection halves fall out of the descriptor limit and the thread
//! half out of the core count, a host with a larger allowance keeps more members
//! resident with **no code change** — the budget tracks the host, never the
//! member count.
//!
//! # Not on the single-root path
//! A budget exists only where a workspace does. [`Backing::Single`](super::Backing::Single)
//! never constructs one, and [`Engine::start`](crate::Engine::start) keeps its
//! core-sized pools exactly as today ([FR-WS-03], [ADR-52]).
//!
//! [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
//! [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
//! [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
//! [ADR-63]: ../../../docs/specs/architecture/decisions/ADR-63.md

/// File descriptors one open SQLite WAL connection holds: the database file, its
/// `-wal`, and its `-shm` mapping ([NFR-RA-10]).
///
/// [NFR-RA-10]: ../../../docs/specs/requirements/NFR-RA-10.md
const FDS_PER_CONNECTION: usize = 3;

/// Descriptors withheld from the budget for everything that is not a member's
/// connection: stdio, the plugin substrate's assets, watcher handles, and the
/// serve socket. Deliberately generous — the cost of over-reserving is a smaller
/// resident set, the cost of under-reserving is the `EMFILE` this budget exists
/// to prevent.
const RESERVED_FDS: usize = 64;

/// Connections a resident member holds *besides* its read pool: the
/// single-writer actor's one RW connection ([ADR-02]).
///
/// [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
const WRITER_CONNECTIONS_PER_MEMBER: usize = 1;

/// Descriptors a resident member holds that are **not** database connections:
/// its filesystem watcher's kqueue/inotify handle under `serve`.
///
/// Charged per member rather than taken from the fixed reserve because it scales
/// with residency — a reserve that covers it at eight residents does not cover it
/// at eight hundred, and residency is exactly what a roomier host is allowed to
/// grow.
const NON_CONNECTION_FDS_PER_MEMBER: usize = 1;

/// Resident members allowed per host core, whatever the descriptor limit.
///
/// Descriptors are not the only resource residency multiplies: each resident
/// engine still owns a hydration cache, and (before [`worker_threads`] made the
/// pool shared) a private thread pool as well. A host with a 65 536-descriptor
/// allowance would otherwise be told it may hold ~1 700 members resident, which
/// is sound in descriptors and absurd in memory. Tying the ceiling to cores keeps
/// it host-derived — never a function of the member count — while the descriptor
/// budget stays the binding constraint on any ordinary host.
///
/// [`worker_threads`]: ConnectionBudget::worker_threads
///
/// [ADR-63]: ../../../docs/specs/architecture/decisions/ADR-63.md
const MAX_RESIDENT_MEMBERS_PER_CORE: usize = 16;

/// Read connections a member is never shrunk below — one connection is the floor
/// at which a member can answer at all.
const MIN_READ_CONNECTIONS_PER_MEMBER: usize = 1;

/// Residency the budget tries to reach before it stops trading away per-member
/// read concurrency.
///
/// Below roughly this many resident members a fan-out that revisits members —
/// `xservice impact` is the motivating pattern — rebuilds engines faster than it
/// uses them. Above it, extra residency buys less than the read concurrency it
/// costs the hot member ([NFR-PE-01]).
///
/// [NFR-PE-01]: ../../../docs/specs/requirements/NFR-PE-01.md
const TARGET_RESIDENT_MEMBERS: usize = 8;

/// Resident members the budget guarantees regardless of how tight the descriptor
/// limit is: a cross-service answer that cannot hold two members at once would
/// rebuild an engine on every comparison.
const MIN_RESIDENT_MEMBERS: usize = 2;

/// Workers the shared pool is never sized below — a pool with no worker cannot
/// run a job at all.
///
/// The clamp is load-bearing rather than cosmetic: `rayon` treats
/// `num_threads(0)` as "choose automatically", so an unclamped zero would not
/// fail, it would silently produce a pool sized by `RAYON_NUM_THREADS` or the
/// host's cores — a thread count the budget never authorised. (The builder now
/// rejects a zero outright, so the two guards agree.)
const MIN_WORKER_THREADS: usize = 1;

/// Soft limit assumed when the platform reports none, matching the stock POSIX
/// default rather than guessing high.
const ASSUMED_FD_SOFT_LIMIT: u64 = 256;

/// Descriptor allowance above which a larger limit buys no more residency.
///
/// Guards the derivation against `RLIM_INFINITY` and against an adversarial
/// argument to [`ConnectionBudget::from_limits`], which is public.
const MAX_USEFUL_FD_SOFT_LIMIT: u64 = 1_048_576;

/// A workspace's ceiling on live read connections, and the residency that
/// ceiling implies ([NFR-PE-11], [ADR-63]).
///
/// Constructed from the host — never from the member count — so `N` is limited
/// by patience rather than by the descriptor table.
///
/// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
/// [ADR-63]: ../../../docs/specs/architecture/decisions/ADR-63.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionBudget {
    per_member_read_connections: usize,
    max_resident_members: usize,
    worker_threads: usize,
}

impl ConnectionBudget {
    /// Derive the budget from **this host**: its `RLIMIT_NOFILE` soft limit and
    /// its core count.
    ///
    /// **Observes** the limit; it does not change it. Raising the soft limit is
    /// process-level startup work the binary owns ([`main`] calls
    /// [`raise_open_file_limit`](crate::fdlimit::raise_open_file_limit) before
    /// anything opens a file), and [ADR-63] scopes the raise to startup for that
    /// reason. A value constructor that mutated a process-global limit as a side
    /// effect would do it behind the back of every embedder of this crate — and
    /// the budget is the guarantee regardless, so there is nothing to gain by it.
    ///
    /// [`main`]: ../../../cli/src/main.rs
    /// [ADR-63]: ../../../docs/specs/architecture/decisions/ADR-63.md
    pub fn from_host() -> Self {
        let soft_limit = crate::fdlimit::open_file_soft_limit().unwrap_or(ASSUMED_FD_SOFT_LIMIT);
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Self::from_limits(soft_limit, cores)
    }

    /// Derive the budget from an explicit descriptor soft limit and core count —
    /// the pure function [`from_host`](Self::from_host) wraps.
    ///
    /// Public so a caller (and the fitness tests behind [NFR-PE-11]) can ask what
    /// a *given* host would budget without having to become that host.
    ///
    /// The derivation, in order: withhold `RESERVED_FDS`, convert the remainder
    /// to whole connections, then shrink the per-member read pool from `cores`
    /// until `TARGET_RESIDENT_MEMBERS` fit — stopping at
    /// `MIN_READ_CONNECTIONS_PER_MEMBER`, because a member with no connection
    /// cannot answer at all. Residency is whatever the surviving pool size
    /// affords, never fewer than `MIN_RESIDENT_MEMBERS`.
    ///
    /// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
    pub fn from_limits(fd_soft_limit: u64, cores: usize) -> Self {
        // An `RLIM_INFINITY` limit is true but useless as a budget input — it
        // would derive an effectively unbounded resident set, reinstating the
        // `members × cores` growth this type exists to remove. Clamped here as
        // well as at the reporting boundary so the public derivation is sound for
        // any argument, not only for the ones this host can report.
        let fd_soft_limit = fd_soft_limit.min(MAX_USEFUL_FD_SOFT_LIMIT);
        let usable_fds = usize::try_from(fd_soft_limit)
            .unwrap_or(usize::MAX)
            .saturating_sub(RESERVED_FDS);
        let budgeted_units = usable_fds / FDS_PER_CONNECTION;

        // The largest per-member read pool that still leaves room for
        // `TARGET_RESIDENT_MEMBERS`, capped at one connection per core and
        // floored at one. This is the closed form of "shrink the pool until the
        // target residency fits": `units_for(u, p) >= TARGET` holds exactly while
        // `p + per_member_overhead() <= u / TARGET`. Solved rather than searched
        // so an adversarial `cores` can neither overflow nor spin.
        let affordable = (budgeted_units / TARGET_RESIDENT_MEMBERS)
            .saturating_sub(per_member_overhead())
            .max(MIN_READ_CONNECTIONS_PER_MEMBER);
        let per_member = cores
            .clamp(MIN_READ_CONNECTIONS_PER_MEMBER, affordable.max(MIN_READ_CONNECTIONS_PER_MEMBER));

        Self {
            per_member_read_connections: per_member,
            max_resident_members: residents_for(budgeted_units, per_member)
                .clamp(MIN_RESIDENT_MEMBERS, host_residency_ceiling(cores)),
            // Threads are budgeted off the core count, not off the descriptor
            // table: a worker thread costs no descriptor, and the resident set
            // shares ONE pool, so the workspace's thread cost is what a single
            // engine would have spawned for itself ([NFR-PE-11], [ADR-63]).
            worker_threads: cores.max(MIN_WORKER_THREADS),
        }
    }

    /// Read connections one resident member engine opens — the value the
    /// registry passes to [`MemberEngine::start`](super::MemberEngine::start).
    pub fn per_member_read_connections(&self) -> usize {
        self.per_member_read_connections
    }

    /// Member engines that may be resident at once. The registry evicts
    /// least-recently-used members to stay at or below this.
    pub fn max_resident_members(&self) -> usize {
        self.max_resident_members
    }

    /// The ceiling on live read connections across the whole workspace — the
    /// quantity [NFR-PE-11] bounds and the tests instrument.
    ///
    /// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
    pub fn total_read_connections(&self) -> usize {
        self.per_member_read_connections * self.max_resident_members
    }

    /// Workers in the **one** `rayon` pool every resident member engine shares —
    /// the workspace's whole thread cost ([NFR-PE-11], [ADR-63]).
    ///
    /// Derived from the host's core count, so it is the same size a single-root
    /// engine builds for itself: federating N members multiplies the *stores* a
    /// query reaches, never the CPU the host has to run them on. Unlike the
    /// connection halves it is independent of the descriptor limit — a thread
    /// costs no descriptor, and the pool is shared rather than per-member, so
    /// residency does not multiply it.
    ///
    /// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
    /// [ADR-63]: ../../../docs/specs/architecture/decisions/ADR-63.md
    pub fn worker_threads(&self) -> usize {
        self.worker_threads
    }
}

/// Descriptor-sized units a resident member costs **besides** its read pool: its
/// writer connection, plus its watcher handle expressed in the same units.
fn per_member_overhead() -> usize {
    WRITER_CONNECTIONS_PER_MEMBER + NON_CONNECTION_FDS_PER_MEMBER
}

/// How many members fit in `budgeted_units` when each holds `per_member` readers
/// plus its per-member overhead.
fn residents_for(budgeted_units: usize, per_member: usize) -> usize {
    budgeted_units / per_member.saturating_add(per_member_overhead())
}

/// The residency this host tolerates whatever its descriptor allowance, so a
/// roomy fd table cannot authorise a resident set the machine's threads and
/// memory could not carry.
fn host_residency_ceiling(cores: usize) -> usize {
    cores
        .max(1)
        .saturating_mul(MAX_RESIDENT_MEMBERS_PER_CORE)
        .max(MIN_RESIDENT_MEMBERS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of [NFR-PE-11]: the budget fits inside the descriptor
    /// limit it was derived from, counting **every** connection a resident
    /// member holds — readers and its writer — at three descriptors each.
    fn assert_fits_within(budget: ConnectionBudget, fd_soft_limit: u64) {
        let connections = budget.max_resident_members()
            * (budget.per_member_read_connections() + WRITER_CONNECTIONS_PER_MEMBER);
        // Every descriptor a full resident set holds: its connections at three
        // apiece, plus each member's watcher handle. Charging the watcher here is
        // what makes this a bound on the OS resource rather than on connections
        // alone — a reserve that covers watchers at eight residents does not
        // cover them at eight hundred.
        let fds = connections * FDS_PER_CONNECTION
            + budget.max_resident_members() * NON_CONNECTION_FDS_PER_MEMBER;
        let limit = usize::try_from(fd_soft_limit).unwrap_or(usize::MAX);
        assert!(
            fds + RESERVED_FDS <= limit,
            "a budget of {} residents × {} readers needs {fds} fds, over the {limit}-fd limit",
            budget.max_resident_members(),
            budget.per_member_read_connections(),
        );
    }

    /// The stock macOS default — the limit the 72-member workspace actually died
    /// on — budgets a resident set that fits inside 256 descriptors.
    #[test]
    fn the_stock_256_fd_limit_budgets_within_itself() {
        let budget = ConnectionBudget::from_limits(256, 12);
        assert_fits_within(budget, 256);
        assert!(
            budget.max_resident_members() >= MIN_RESIDENT_MEMBERS,
            "even the stock limit must hold two members at once"
        );
        assert!(
            budget.per_member_read_connections() < 12,
            "a 12-core read pool per member cannot fit; the pool must shrink"
        );
    }

    /// Eviction is driven by the **budget**, not by a fixed member count: a host
    /// with a larger descriptor allowance keeps more members resident with no
    /// code change ([NFR-PE-11] acceptance).
    #[test]
    fn a_larger_fd_allowance_keeps_more_members_resident() {
        let tight = ConnectionBudget::from_limits(256, 12);
        let roomy = ConnectionBudget::from_limits(65_536, 12);
        assert!(
            roomy.max_resident_members() > tight.max_resident_members(),
            "a 65k-fd host must hold more members than a 256-fd host \
             ({} vs {})",
            roomy.max_resident_members(),
            tight.max_resident_members(),
        );
        assert!(
            roomy.total_read_connections() > tight.total_read_connections(),
            "the connection ceiling must scale with the host too"
        );
        assert_fits_within(roomy, 65_536);
    }

    /// A roomy host keeps the full core-sized read pool per member — the budget
    /// only trades read concurrency away under descriptor pressure ([NFR-PE-01]).
    #[test]
    fn a_roomy_host_keeps_the_core_sized_read_pool() {
        let budget = ConnectionBudget::from_limits(65_536, 12);
        assert_eq!(
            budget.per_member_read_connections(),
            12,
            "with descriptors to spare a member keeps its core-sized pool"
        );
    }

    /// The budget never collapses to zero, however hostile the limit: a member
    /// keeps one connection and the workspace keeps two members, because a
    /// registry that can hold neither cannot answer a cross-service query at all.
    #[test]
    fn a_hostile_fd_limit_still_budgets_a_usable_floor() {
        // Limits at or below the reserve leave nothing to divide up, so the
        // floor is all there is.
        for limit in [0, 1, RESERVED_FDS as u64] {
            let budget = ConnectionBudget::from_limits(limit, 12);
            assert_eq!(
                budget.per_member_read_connections(),
                MIN_READ_CONNECTIONS_PER_MEMBER,
                "under pressure the read pool shrinks to its floor, not below"
            );
            assert_eq!(
                budget.max_resident_members(),
                MIN_RESIDENT_MEMBERS,
                "the workspace keeps its two-member floor at limit {limit}"
            );
        }

        // Just above the reserve there is room for a handful of one-reader
        // members — still the read-pool floor, but residency is earned, not
        // floored.
        let budget = ConnectionBudget::from_limits(96, 12);
        assert_eq!(
            budget.per_member_read_connections(),
            MIN_READ_CONNECTIONS_PER_MEMBER
        );
        assert!(budget.max_resident_members() >= MIN_RESIDENT_MEMBERS);
        assert_fits_within(budget, 96);
    }

    /// Where the two-resident floor stops fitting.
    ///
    /// `MIN_RESIDENT_MEMBERS` is a floor applied *after* the affordability
    /// arithmetic, so below a certain limit the budget knowingly over-commits —
    /// two members that cannot answer at all is worse than two that might. This
    /// pins the exact boundary, so a change to `RESERVED_FDS` or to the floor
    /// moves it visibly rather than silently.
    #[test]
    fn the_two_member_floor_over_commits_only_below_its_own_boundary() {
        // The first limit at which the floor genuinely fits: 2 × (1+1) × 3 + 2
        // watcher fds = 14, plus the 64-fd reserve.
        const FLOOR_FITS_FROM: u64 = 78;
        assert_fits_within(
            ConnectionBudget::from_limits(FLOOR_FITS_FROM, 12),
            FLOOR_FITS_FROM,
        );
        for limit in [0, 1, 64, FLOOR_FITS_FROM - 1] {
            let budget = ConnectionBudget::from_limits(limit, 12);
            assert_eq!(
                budget.max_resident_members(),
                MIN_RESIDENT_MEMBERS,
                "below the boundary the floor is all there is, at limit {limit}"
            );
        }
    }

    /// The derived numbers themselves, pinned exactly for two representative
    /// hosts.
    ///
    /// Every other test here asserts an inequality or a floor, which leaves the
    /// tuning constants free to drift: `TARGET_RESIDENT_MEMBERS` could be halved
    /// or doubled with the whole suite green. These two equalities are what make
    /// a change to the derivation a deliberate act — if one fires, re-derive it
    /// on purpose rather than adjusting the number to match.
    #[test]
    fn the_derived_budget_is_pinned_for_representative_hosts() {
        let stock = ConnectionBudget::from_limits(256, 12);
        assert_eq!(
            (
                stock.per_member_read_connections(),
                stock.max_resident_members()
            ),
            (6, 8),
            "the stock 256-fd / 12-core host"
        );
        // Literal, not constant-derived: 8 residents × (6 readers + 1 writer) × 3
        // fds + 8 watcher fds = 176, inside 256 - 64 reserved = 192.
        assert_eq!(8 * (6 + 1) * 3 + 8, 176);

        let roomy = ConnectionBudget::from_limits(65_536, 12);
        assert_eq!(
            (
                roomy.per_member_read_connections(),
                roomy.max_resident_members()
            ),
            (12, 192),
            "a 65k-fd / 12-core host keeps its core-sized pool, capped by cores"
        );
    }

    /// Residency is bounded by the host, never by the member count. The ceiling
    /// scales with **cores**, so a descriptor allowance far beyond what the
    /// machine can carry in threads and memory does not authorise a resident set
    /// to match it.
    #[test]
    fn residency_is_ceilinged_by_the_host_not_by_its_fd_table() {
        let huge = ConnectionBudget::from_limits(u64::MAX, 12);
        assert_eq!(
            huge.max_resident_members(),
            host_residency_ceiling(12),
            "an unbounded fd table must not derive an unbounded resident set"
        );
        assert!(
            huge.max_resident_members() < 1_000,
            "a limit of u64::MAX derived {} residents — the ceiling is not holding",
            huge.max_resident_members()
        );
        // More cores carry more residents; the same cores carry the same number
        // whatever the descriptor allowance above the useful maximum.
        assert!(
            ConnectionBudget::from_limits(u64::MAX, 24).max_resident_members()
                > huge.max_resident_members()
        );
        assert_eq!(
            ConnectionBudget::from_limits(MAX_USEFUL_FD_SOFT_LIMIT, 12),
            huge,
            "beyond the useful maximum a larger limit buys nothing"
        );
    }

    /// `from_limits` is public and takes an arbitrary `cores`, so it must return
    /// a usable budget for every input rather than panicking, overflowing, or
    /// spinning — including the degenerate `cores = 0` and the adversarial
    /// `usize::MAX` that a search-based derivation would hang on.
    #[test]
    fn any_input_yields_a_usable_budget() {
        for limit in [0, 1, 64, 75, 76, 96, 256, 4096, 65_536, u64::MAX] {
            for cores in [0, 1, 12, 128, usize::MAX] {
                let budget = ConnectionBudget::from_limits(limit, cores);
                assert!(
                    budget.per_member_read_connections() >= MIN_READ_CONNECTIONS_PER_MEMBER,
                    "limit {limit} / cores {cores} budgeted a member zero connections, \
                     which the reader pool rejects outright"
                );
                assert!(
                    budget.max_resident_members() >= MIN_RESIDENT_MEMBERS,
                    "limit {limit} / cores {cores} budgeted fewer than two residents"
                );
                assert_eq!(
                    budget.total_read_connections(),
                    budget.per_member_read_connections() * budget.max_resident_members(),
                    "limit {limit} / cores {cores} overflowed its own ceiling"
                );
            }
        }
    }

    /// A single-core host still budgets a working read pool.
    #[test]
    fn a_single_core_host_budgets_one_reader_per_member() {
        let budget = ConnectionBudget::from_limits(4096, 1);
        assert_eq!(budget.per_member_read_connections(), 1);
        assert!(budget.max_resident_members() > TARGET_RESIDENT_MEMBERS);
    }

    /// The host-derived budget is self-consistent — whatever this host reports,
    /// the ceiling is the product of its two halves and both are non-zero.
    #[test]
    fn the_host_budget_is_non_degenerate() {
        let budget = ConnectionBudget::from_host();
        assert!(budget.per_member_read_connections() >= MIN_READ_CONNECTIONS_PER_MEMBER);
        assert!(budget.max_resident_members() >= MIN_RESIDENT_MEMBERS);
        assert_eq!(
            budget.total_read_connections(),
            budget.per_member_read_connections() * budget.max_resident_members()
        );
    }

    /// The thread half of the budget ([NFR-PE-11], S-325): sized by the host's
    /// cores, never by the descriptor limit and never by the member count.
    #[test]
    fn worker_threads_track_the_cores_and_nothing_else() {
        for fd_limit in [64, 256, 4_096, 65_536, u64::MAX] {
            for cores in [1, 2, 12, 64] {
                let budget = ConnectionBudget::from_limits(fd_limit, cores);
                assert_eq!(
                    budget.worker_threads(),
                    cores,
                    "a {cores}-core host under a {fd_limit}-fd limit budgeted \
                     {} worker threads; a thread costs no descriptor, so the \
                     limit must not enter this",
                    budget.worker_threads(),
                );
            }
        }
    }

    /// The pool is shared, so the workspace's thread cost is a *single* engine's
    /// — it does not rise with residency the way connections do.
    #[test]
    fn worker_threads_do_not_multiply_by_residency() {
        let tight = ConnectionBudget::from_limits(256, 12);
        let roomy = ConnectionBudget::from_limits(65_536, 12);
        assert!(
            roomy.max_resident_members() > tight.max_resident_members(),
            "fixture assumption: the roomy host holds more members resident"
        );
        assert_eq!(
            roomy.worker_threads(),
            tight.worker_threads(),
            "holding more members resident must not cost more worker threads; \
             that is the multiple the shared pool removes"
        );
    }

    /// A degenerate host still gets a pool it can run a job on.
    ///
    /// A construction precondition, not taste: without the clamp the budget would
    /// report zero worker threads while `rayon` quietly ran a host-sized pool, so
    /// the number [NFR-PE-11] bounds would stop describing the process.
    #[test]
    fn a_coreless_host_still_gets_one_worker() {
        assert_eq!(ConnectionBudget::from_limits(256, 0).worker_threads(), 1);
    }
}
