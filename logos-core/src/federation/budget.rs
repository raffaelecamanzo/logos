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
//! ceiling**. It answers two questions the [registry](super::EngineRegistry) asks:
//! how many read connections one resident member may open
//! ([`per_member_read_connections`](ConnectionBudget::per_member_read_connections)),
//! and how many members may be resident at once
//! ([`max_resident_members`](ConnectionBudget::max_resident_members)). Their
//! product is the ceiling on live read connections
//! ([`total_read_connections`](ConnectionBudget::total_read_connections)) — the
//! quantity [NFR-PE-11] bounds.
//!
//! Because both fall out of the descriptor limit, a host with a larger allowance
//! keeps more members resident with **no code change** — the budget tracks the
//! host, never the member count.
//!
//! # Not on the single-root path
//! A budget exists only where a workspace does. [`Backing::Single`](super::Backing::Single)
//! never constructs one, and [`Engine::start`](crate::Engine::start) keeps its
//! core-sized pool exactly as today ([FR-WS-03], [ADR-52]).
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

/// Soft limit assumed when the platform reports none, matching the stock POSIX
/// default rather than guessing high.
const ASSUMED_FD_SOFT_LIMIT: u64 = 256;

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
}

impl ConnectionBudget {
    /// Derive the budget from **this host**: its `RLIMIT_NOFILE` soft limit and
    /// its core count.
    ///
    /// Raises the soft limit toward the hard limit first
    /// ([`raise_open_file_limit`](crate::fdlimit::raise_open_file_limit)) so the
    /// budget is derived from the widest limit the kernel will grant — a
    /// best-effort, secondary defence the budget itself never depends on
    /// ([ADR-63]).
    ///
    /// [ADR-63]: ../../../docs/specs/architecture/decisions/ADR-63.md
    pub fn from_host() -> Self {
        let soft_limit = crate::fdlimit::raise_open_file_limit().unwrap_or(ASSUMED_FD_SOFT_LIMIT);
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
        let usable_fds = usize::try_from(fd_soft_limit)
            .unwrap_or(usize::MAX)
            .saturating_sub(RESERVED_FDS);
        let connections = usable_fds / FDS_PER_CONNECTION;

        let mut per_member = cores.max(MIN_READ_CONNECTIONS_PER_MEMBER);
        while per_member > MIN_READ_CONNECTIONS_PER_MEMBER
            && residents_for(connections, per_member) < TARGET_RESIDENT_MEMBERS
        {
            per_member -= 1;
        }

        Self {
            per_member_read_connections: per_member,
            max_resident_members: residents_for(connections, per_member).max(MIN_RESIDENT_MEMBERS),
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
}

/// How many members fit in `connections` when each holds `per_member` readers
/// plus its writer connection.
fn residents_for(connections: usize, per_member: usize) -> usize {
    connections / (per_member + WRITER_CONNECTIONS_PER_MEMBER)
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
        let fds = connections * FDS_PER_CONNECTION;
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

    /// Residency is bounded by the host, never by the member count — the same
    /// host budgets the same ceiling for a 72-member and a 200-member workspace.
    #[test]
    fn the_budget_does_not_depend_on_the_member_count() {
        // There is no member count in the constructor at all; this pins that
        // the derivation's only inputs stay (fd limit, cores).
        let a = ConnectionBudget::from_limits(256, 12);
        let b = ConnectionBudget::from_limits(256, 12);
        assert_eq!(a, b);
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
}
