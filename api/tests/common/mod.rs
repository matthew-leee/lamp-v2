//! Shared D2c scratch-database harness for integration tests.
//!
//! This module is NOT a test binary: it contains no `#[test]` and is compiled
//! into whichever test binaries declare `mod common;`. It holds two layers:
//! the scratch-database harness below, giving each DB-backed test its own
//! throwaway database on the D2c scratch Postgres server (port 55433) without
//! ever touching the shared databases:
//!
//! * `scratch_db_url()` is the env gate (`STOREFRONT_TEST_DATABASE_URL`). When the
//!   variable is unset the DB tests skip, with one loud banner so the skip is
//!   visible in plain `cargo test` output. When it IS set, the URL must pass
//!   [`validate_scratch_target`]: a set-but-wrong target panics — it never
//!   silently downgrades to a skip.
//! * [`validate_scratch_target`] is a pure, network-free endpoint ALLOWLIST
//!   (see its docs for why an allowlist and not a denylist). The only
//!   sanctioned target is the D2c scratch server's loopback TCP endpoint on
//!   port 55433; port 55432 is the storefront SHARED verify database and D2c
//!   tests must never race it — connecting there is a hard panic, not a
//!   warning. `ScratchDb::new` re-validates at the constructor boundary, so
//!   safety never depends on callers remembering the gate.
//! * [`assert_ambient_pg_env_clean`] refuses a run whose ambient `PGOPTIONS`
//!   would poison every connection (sqlx merges it at parse time; no public
//!   API can clear it) — enforced at the same constructor boundary.
//! * `ScratchDb` creates a uniquely-named ephemeral database, hands out a pool
//!   for queries, replays migrations from disk, and drops the database again in
//!   `Drop` (bounded by a timeout), so a test run leaves the scratch server
//!   clean even when a test panics mid-way. If the test pool cannot attach
//!   after CREATE DATABASE, the constructor drops the database again before
//!   panicking — a constructor failure must not leak a database.
//!
//! Admin-suite machinery layer (bottom section, extracted from
//! `d2c_admin_auth.rs` in D2c step 4): the boot / router / cookie / request
//! helpers, the ambient-env guards and the DB-witness queries that the admin
//! suites (`d2c_admin_auth`, `d2c_admin_mutations`) both drive — one router
//! and one boot definition so the suites can never drift apart. Suite-specific
//! fixtures, probe constants and exact-body strings stay in the suite files.
//! Not every test binary exercises every helper, and that is expected here.

// `common` is shared across several test binaries and each binary exercises a
// different subset of it (e.g. `d2c_migration_harness` drives the harness but
// none of the admin machinery, `d2c_admin_auth` does not need the
// locale-env guard), so "never used" is EXPECTED for some items in some
// binaries and must not fail the `-D warnings` gate. This allow covers only
// items DEFINED in this shared module — suite-local code stays fully linted.
#![allow(dead_code)]

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Once};
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::routing::{delete, get, patch, post, put};
use axum::{middleware, Router};
use chrono::{DateTime, Utc};
use lamp_api::admin::{self, Rotation, SessionKey};
use lamp_api::state::AppState;
use lamp_api::{admin_crud, content, orders};
use serde_json::Value;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions, PgSslMode};
use tokio::sync::Semaphore;
use tower::ServiceExt; // oneshot

/// Env gate for every DB-backed test. Expected value shape:
/// `postgres://<user>:<password>@localhost:55433/<database>` (the D2c scratch
/// server). Unset means "skip DB tests", never "run against something else".
pub const DATABASE_URL_ENV: &str = "STOREFRONT_TEST_DATABASE_URL";

/// The storefront shared verify DB port. D2c tests create and DROP sibling
/// databases on whatever server the URL points at, so pointing them at the
/// shared verify DB would race the storefront's verification runs. Refused.
const SHARED_VERIFY_DB_PORT: u16 = 55432;

/// The D2c scratch server port: the only sanctioned
/// target for these tests. The scratch recipe pins 55433 exactly; changing it
/// is a deliberate, documented edit of THIS constant (and the recipe) — never
/// something a stray URL or ambient configuration can renegotiate.
const SCRATCH_DB_PORT: u16 = 55433;

/// Per-session server-side `statement_timeout` applied to EVERY harness
/// connection (both pools), so a pathological migration or query fails in
/// seconds instead of hanging a test on a stuck server.
const STATEMENT_TIMEOUT: &str = "10s";

/// Wall-clock bound on the whole `Drop` cleanup future (pool close + DROP
/// DATABASE). Beyond this the run reports the database by exact name for
/// manual cleanup instead of hanging the test process.
const DROP_CLEANUP_TIMEOUT: Duration = Duration::from_secs(30);

/// Migration replay order, explicit on purpose. Filesystem/directory order is
/// an implementation detail of `fs::read_dir`; the schema depends on 0001
/// before 0002 (ALTER TABLE) before 0003 (seed) before 0004 (admin) before
/// 0005 (flagship media/spec columns + the refined copy) before 0006 (orders /
/// payment_events: the money path references `products`, so it can never land
/// earlier). If a new migration lands, it must be appended here deliberately —
/// a test that silently picks up 0007 and fails later is harder to diagnose
/// than one that keeps replaying the verified 0001..0006 contract until
/// updated.
const MIGRATIONS: &[&str] = &[
    "0001_init.sql",
    "0002_catalogue_content.sql",
    "0003_seed_opening_drop.sql",
    "0004_admin.sql",
    "0005_flagship_media_specs.sql",
    "0006_orders_reservations.sql",
    "0007_multi_sku_fps_media.sql",
    "0008_payment_email_session_hardening.sql",
];

/// Where the migration files live on disk, resolved at compile time so the
/// tests work regardless of the process working directory.
const MIGRATIONS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/migrations");

/// Loud skip banner. Printed exactly once per test-binary run, straight to the
/// stderr handle rather than via `eprintln!`: libtest captures the `print!`
/// macro family for PASSING tests and only replays it for failures or under
/// `--nocapture`, so a plain `eprintln!` would be invisible in the default
/// `cargo test` output that must show this skip.
const SKIP_BANNER: &str = "\
================================================================================
 D2c DB TESTS SKIPPED: STOREFRONT_TEST_DATABASE_URL is not set.
 Expected value shape: postgres://<user>:<password>@localhost:55433/<database>
 (the D2c scratch Postgres on port 55433; port 55432 is the storefront shared
 verify DB and is refused by the harness endpoint allowlist, never raced by
 D2c tests.)
================================================================================";

static SKIP_BANNER_PRINTED: Once = Once::new();

/// Gate for DB-backed tests: `Some(url)` when the harness is armed AND the
/// URL passes [`validate_scratch_target`], `None` when the caller must skip
/// (banner already printed). A URL that is SET but fails validation panics:
/// set-but-wrong-target must fail loud and early, never silently skip.
pub fn scratch_db_url() -> Option<String> {
    match std::env::var(DATABASE_URL_ENV) {
        Ok(url) if !url.is_empty() => {
            validate_scratch_target(&url).unwrap_or_else(|e| {
                panic!(
                    "{DATABASE_URL_ENV} is set but is not the sanctioned D2c \
                     scratch target: {e}"
                )
            });
            Some(url)
        }
        // Unset or empty both mean "skip": an empty string is never a valid
        // URL, and silently skipping is the honest report for either.
        _ => {
            announce_skip();
            None
        }
    }
}

/// Endpoint ALLOWLIST for the D2c scratch harness — pure and network-free:
/// it only parses and inspects the URL, never connects.
///
/// WHY AN ALLOWLIST, NOT A DENYLIST: the safety property is the IDENTITY of
/// the endpoint, not the absence of one port number. A denylist names only the
/// verify DB's mapped port, but the storefront's shared verify CLUSTER is
/// reachable in ways a denylist waves through — its native 5432 port, another
/// docker/host port mapping onto that same container, a Unix socket path —
/// and every one of those endpoints still races the storefront's verification
/// runs (the scratch server is a SEPARATE container; what matters is that the
/// URL points at it and at nothing else). So the check names the ONE
/// endpoint that is allowed (loopback TCP host AND port 55433, exactly as the
/// scratch recipe pins it) and refuses everything else. Changing the pinned
/// port is a deliberate, documented edit of [`SCRATCH_DB_PORT`], not something
/// a stray URL can renegotiate.
///
/// Returns the reason for a refusal as `Err` (naming the target by
/// host/port/database only — connection strings can carry the password, so
/// they never get logged).
pub fn validate_scratch_target(url: &str) -> Result<(), String> {
    let opts = PgConnectOptions::from_str(url).map_err(|e| {
        format!(
            "did not parse as a postgres URL (expected shape \
             postgres://…@localhost:{SCRATCH_DB_PORT}/…): {e}"
        )
    })?;

    // A Unix-socket URL (`postgres://%2Fpath/…` or `…?host=/path`) reaches a
    // server with no TCP host/port at all — and `get_host()` still reports the
    // env-seeded default, which can look perfectly loopback. The socket path
    // IS the endpoint identity there, so refuse it outright.
    if opts.get_socket().is_some() {
        return Err(format!(
            "target is a Unix socket path, not the sanctioned scratch TCP \
             endpoint (expected host localhost/127.0.0.1/::1 port \
             {SCRATCH_DB_PORT}, e.g. \
             postgres://storefront:scratch-only@localhost:{SCRATCH_DB_PORT}/storefront_scratch)"
        ));
    }

    let target = format!(
        "{}:{}/{}",
        opts.get_host(),
        opts.get_port(),
        opts.get_database().unwrap_or("postgres")
    );

    // `get_host()` yields the URL's host text as written: `localhost`,
    // `127.0.0.1`, or the bracketed IPv6 literal `[::1]` (a `?host=` /
    // `hostaddr=` form carries whatever text it was given). Trim the IPv6
    // brackets and compare hostnames case-insensitively; parse as an IP so
    // every spelling of the loopback ADDRESSES passes while e.g. 127.0.0.2
    // does not.
    let host = opts
        .get_host()
        .trim_start_matches('[')
        .trim_end_matches(']');
    let loopback = host.eq_ignore_ascii_case("localhost")
        || match host.parse::<IpAddr>() {
            Ok(IpAddr::V4(v4)) => v4 == Ipv4Addr::LOCALHOST,
            Ok(IpAddr::V6(v6)) => v6 == Ipv6Addr::LOCALHOST,
            Err(_) => false,
        };

    if opts.get_port() == SHARED_VERIFY_DB_PORT {
        return Err(format!(
            "{target} is the storefront shared verify DB (port \
             {SHARED_VERIFY_DB_PORT}); D2c tests create and DROP whole \
             databases and must never race the storefront's verification \
             runs. Use the D2c scratch server recipe instead: \
             postgres://storefront:scratch-only@localhost:{SCRATCH_DB_PORT}/storefront_scratch"
        ));
    }
    if !loopback {
        return Err(format!(
            "{target} is not the sanctioned D2c scratch endpoint (host must \
             be localhost/127.0.0.1/::1): use the scratch recipe \
             postgres://storefront:scratch-only@localhost:{SCRATCH_DB_PORT}/storefront_scratch"
        ));
    }
    if opts.get_port() != SCRATCH_DB_PORT {
        return Err(format!(
            "{target} is not the sanctioned D2c scratch endpoint (port must \
             be {SCRATCH_DB_PORT}): use the scratch recipe \
             postgres://storefront:scratch-only@localhost:{SCRATCH_DB_PORT}/storefront_scratch"
        ));
    }
    Ok(())
}

/// The one ambient Postgres env var the harness refuses: sqlx 0.8.x merges
/// `PGOPTIONS` into EVERY connection's startup options while parsing the URL
/// (the parse inherits the process environment), and no public API can clear
/// the merged options afterwards — `.options()` only APPENDS. An ambient
/// `-c search_path=…` would poison DDL; an ambient `-c statement_timeout=…`
/// could unbound or skew the fixture; so a set value fails loud instead of
/// running a silently skewed harness. (The other PG* variables that reach
/// connections are neutralized or bounded: `PGSSLMODE`/`PGSSLROOTCERT`/…
/// are overridden by the explicit `.ssl_mode(Disable)` in
/// [`ScratchDb::new`], and connect waits are bounded by the pool's
/// `acquire_timeout`.)
const AMBIENT_PGOPTIONS_ENV: &str = "PGOPTIONS";

/// Fail-loud ambient check: panics naming the variable to unset.
///
/// Pure over `lookup` so tests drive a mock environment: mutating the
/// process-wide environment from a test would race the parallel test
/// threads. [`ScratchDb::new`] wires it to the live environment at the
/// constructor boundary, right where the first connection is about to
/// exist.
pub fn assert_ambient_pg_env_clean(lookup: impl Fn(&str) -> Option<String>) {
    if let Some(raw) = lookup(AMBIENT_PGOPTIONS_ENV) {
        panic!(
            "ambient {AMBIENT_PGOPTIONS_ENV} is set ({raw:?}): sqlx merges it \
             into every connection's startup options and no public API can \
             clear it; unset it before running the D2c harness"
        );
    }
}

/// Print the skip banner exactly once per process (tests run in parallel).
fn announce_skip() {
    SKIP_BANNER_PRINTED.call_once(|| {
        use std::io::Write;
        // Direct stderr write, deliberately not `eprintln!` — see SKIP_BANNER.
        let mut err = std::io::stderr();
        let _ = writeln!(err, "{SKIP_BANNER}");
    });
}

/// One ephemeral scratch database, dropped again on [`Drop`].
///
/// The struct owns a private tokio runtime. That is not decoration: `Drop` is
/// synchronous but must `close()` a sqlx pool and run `DROP DATABASE`, and
/// `Pool::close()` only completes when the pool's connection tasks can wind
/// down. If those tasks lived on a `#[tokio::test]` executor we would have to
/// block that executor to wait for them (a deadlock, or a "cannot block from
/// within a runtime" panic). Keeping the pools on our own runtime lets the
/// tests stay plain `#[test]` fns and drive queries through
/// [`ScratchDb::block_on`], with no cross-runtime hazards.
pub struct ScratchDb {
    /// Ephemeral database name, e.g. `d2c_test_admin_edits_1234`. Only ever
    /// used as an SQL identifier in CREATE/DROP DATABASE — it is generated
    /// from a validated slug + pid, never taken from user input.
    db_name: String,
    /// Pool on the ephemeral database: this is what tests query.
    pool: PgPool,
    /// Maintenance pool, connected to the database named by the URL (the
    /// scratch `storefront_scratch`): CREATE/DROP DATABASE are server-level
    /// commands, so they run here and NEVER touch that database's own tables.
    admin_pool: PgPool,
    /// Private runtime that owns the connection tasks of both pools.
    rt: tokio::runtime::Runtime,
}

impl ScratchDb {
    /// Creates `d2c_test_<slug>_<pid>` on the scratch server.
    ///
    /// `slug` distinguishes tests running in parallel inside one test binary
    /// (same pid); the pid distinguishes concurrent test binaries/processes.
    /// A stale database left by a crashed run is reclaimed ONLY when its name
    /// gets reused (same pid + slug) by the `DROP DATABASE IF EXISTS ... WITH
    /// (FORCE)` before the create — a different pid never sweeps it, which is
    /// why the fail-path cleanup below and the bounded `Drop` matter.
    ///
    /// Fails as a panic with no database leaked: a wrong target and a set
    /// ambient `PGOPTIONS` are both refused before any connection, and a
    /// test pool that cannot attach after CREATE DATABASE triggers a
    /// best-effort DROP before the panic.
    pub fn new(url: &str, slug: &str) -> ScratchDb {
        // Constructor-boundary enforcement: safety must not depend on callers
        // remembering the env gate — refuse a wrong target FIRST, before any
        // connection exists.
        validate_scratch_target(url)
            .unwrap_or_else(|e| panic!("D2c harness: refusing to create a scratch database: {e}"));
        // Ambient startup-options guard: `PGOPTIONS` would ride along on
        // every connection (sqlx merges it at parse time; see
        // [`assert_ambient_pg_env_clean`]), so refuse it before the first
        // connection can exist.
        assert_ambient_pg_env_clean(|name| std::env::var(name).ok().filter(|v| !v.is_empty()));
        assert!(
            slug.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                && !slug.is_empty(),
            "ScratchDb slug must be a non-empty [a-z0-9_] string (it becomes a \
             database identifier): {slug:?}"
        );
        let opts = PgConnectOptions::from_str(url).unwrap_or_else(|e| {
            panic!(
                "{DATABASE_URL_ENV} did not parse as a postgres URL \
                 (expected postgres://…@localhost:{SCRATCH_DB_PORT}/…): {e}"
            )
        });
        // Explicit execution bounds on EVERY connection (both pools):
        //
        // * `ssl_mode(Disable)`: sqlx seeds connect defaults from the ambient
        //   libpq environment (PGSSLMODE, PGSSLROOTCERT, … — see
        //   `PgConnectOptions::new_without_pgpass`), so a developer's
        //   verify-full setup could silently demand TLS from the scratch
        //   container and fail the harness in confusing ways. The scratch
        //   server is plain TCP on localhost; pin SSL OFF so the ambient env
        //   can never reach in.
        // * `statement_timeout`: per-session server parameter (delivered via
        //   the startup `options`) bounding every statement on every harness
        //   connection to [`STATEMENT_TIMEOUT`].
        let opts = opts
            .ssl_mode(PgSslMode::Disable)
            .options([("statement_timeout", STATEMENT_TIMEOUT)]);
        // Error messages below name the target by host/port/database only:
        // connection strings can carry the password, so they never get logged.
        let target = format!(
            "{}:{}/{}",
            opts.get_host(),
            opts.get_port(),
            opts.get_database().unwrap_or("postgres")
        );
        let db_name = format!("d2c_test_{slug}_{}", std::process::id());

        // Multi-thread runtime with one worker: `block_on` below drives futures
        // on the calling thread while the worker runs the pool connection
        // tasks in the background — exactly what `Drop` needs later.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("d2c-scratch-db")
            .enable_all()
            .build()
            .expect("D2c harness: failed to build the scratch-db runtime");

        // The pools come back as values; `rt` is moved into the struct only
        // after `block_on` returns, so the borrow in `rt.block_on(...)` and the
        // struct's ownership never overlap.
        let (admin_pool, pool) = rt.block_on(async {
            // Tiny pools, fail-fast acquire: many ephemeral databases share one
            // scratch server, and a dead server must fail the test in seconds
            // rather than hang it on the default 30s connect timeout.
            let admin_pool = PgPoolOptions::new()
                .max_connections(2)
                .acquire_timeout(Duration::from_secs(5))
                .connect_with(opts.clone())
                .await
                .unwrap_or_else(|e| {
                    panic!("D2c harness: cannot connect admin pool to {target}: {e}")
                });

            sqlx::raw_sql(&format!("DROP DATABASE IF EXISTS {db_name} WITH (FORCE)"))
                .execute(&admin_pool)
                .await
                .unwrap_or_else(|e| {
                    panic!("D2c harness: DROP DATABASE IF EXISTS failed for {db_name}: {e}")
                });
            sqlx::raw_sql(&format!("CREATE DATABASE {db_name}"))
                .execute(&admin_pool)
                .await
                .unwrap_or_else(|e| {
                    panic!("D2c harness: CREATE DATABASE failed for {db_name}: {e}")
                });

            let pool = match PgPoolOptions::new()
                .max_connections(2)
                .acquire_timeout(Duration::from_secs(5))
                .connect_with(opts.database(&db_name))
                .await
            {
                Ok(pool) => pool,
                Err(e) => {
                    // Fail-path cleanup: the database exists but no test pool
                    // could attach. Drop it again (best effort) BEFORE
                    // panicking — a constructor failure must not leak a
                    // database onto the scratch server.
                    if let Err(drop_err) =
                        sqlx::raw_sql(&format!("DROP DATABASE IF EXISTS {db_name} WITH (FORCE)"))
                            .execute(&admin_pool)
                            .await
                    {
                        eprintln!(
                            "D2c harness: fail-path cleanup could not drop \
                             {db_name} on {target}: {drop_err} — drop it \
                             manually: DROP DATABASE IF EXISTS {db_name} WITH (FORCE)"
                        );
                    }
                    panic!("D2c harness: cannot connect test pool to {target} ({db_name}): {e}");
                }
            };

            (admin_pool, pool)
        });

        ScratchDb {
            db_name,
            pool,
            admin_pool,
            rt,
        }
    }

    /// The pool on the ephemeral database, for test queries.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Drives a future to completion on the harness runtime.
    ///
    /// Tests are plain `#[test]` fns, not `#[tokio::test]`: `Drop` calls
    /// `Runtime::block_on`, which panics inside an asynchronous execution
    /// context, so the test bodies must stay synchronous and await through
    /// this helper instead.
    pub fn block_on<F: Future>(&self, fut: F) -> F::Output {
        self.rt.block_on(fut)
    }

    /// Replays the migration set 0001..0006 in explicit order
    /// ([`MIGRATIONS`]). Panics naming the file that failed.
    pub fn apply_migrations(&self) {
        for file_name in MIGRATIONS {
            self.apply_migration_file(file_name);
        }
    }

    /// Simulated-outage seam (D2c step 5, shared by the resilience suite's
    /// drop legs): `DROP DATABASE ... WITH (FORCE)` via the maintenance pool,
    /// which severs every backend of the ephemeral database — and with them
    /// every connection of [`Self::pool`] — exactly like a database outage.
    ///
    /// The pool OBJECT survives (tests keep holding the same handle): sqlx
    /// reconnects lazily, so after [`Self::recreate_database`] a later query
    /// on [`Self::pool`] transparently opens a fresh connection. That lazy
    /// reconnect is not hand-waving: `test_before_acquire` defaults to true
    /// in sqlx 0.8, so dead idle connections are pinged away on the next
    /// acquire and replaced. That is the pool-recovery property the
    /// resilience suite pins: same process, same pool object, outage
    /// visible (503), recovery visible (200).
    pub fn drop_database_with_force(&self) {
        self.block_on(async {
            sqlx::raw_sql(&format!(
                "DROP DATABASE IF EXISTS {} WITH (FORCE)",
                self.db_name
            ))
            .execute(&self.admin_pool)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "D2c harness: DROP DATABASE WITH (FORCE) failed for {}: {e}",
                    self.db_name
                )
            });
        });
    }

    /// Recreates the ephemeral database after [`Self::drop_database_with_force`]
    /// (DROP IF EXISTS + CREATE — the constructor's idempotent sequence, so
    /// the call is safe whatever the current state). Deliberately does NOT
    /// re-apply migrations: the caller decides what the recreated database
    /// must contain ([`Self::apply_migrations`] then runs on the SAME test
    /// pool, which reconnects lazily — see [`Self::drop_database_with_force`]).
    pub fn recreate_database(&self) {
        self.block_on(async {
            sqlx::raw_sql(&format!(
                "DROP DATABASE IF EXISTS {} WITH (FORCE)",
                self.db_name
            ))
            .execute(&self.admin_pool)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "D2c harness: DROP DATABASE IF EXISTS failed for {}: {e}",
                    self.db_name
                )
            });
            sqlx::raw_sql(&format!("CREATE DATABASE {}", self.db_name))
                .execute(&self.admin_pool)
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "D2c harness: CREATE DATABASE failed for {}: {e}",
                        self.db_name
                    )
                });
        });
    }

    /// Replays a single migration file from `api/migrations/` by name (used
    /// directly by the seed-idempotence tests to re-run 0003 on an already
    /// migrated database).
    ///
    /// The file's full contents go through `sqlx::raw_sql`: one simple-protocol
    /// batch, no client-side statement splitting. Postgres wraps such a batch
    /// in an implicit transaction, so a mid-file error aborts and rolls the
    /// whole file back — the fail-loud, stop-at-first-error behavior the
    /// migration contract wants. Any error then panics naming the file.
    pub fn apply_migration_file(&self, file_name: &str) {
        let path = format!("{MIGRATIONS_DIR}/{file_name}");
        let sql = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("D2c harness: cannot read migration {path}: {e}"));
        self.block_on(async {
            sqlx::raw_sql(&sql)
                .execute(&self.pool)
                .await
                .unwrap_or_else(|e| {
                    panic!("D2c harness: migration {file_name} failed to apply: {e}")
                });
        });
    }
}

impl Drop for ScratchDb {
    /// Closes the test pool FIRST (so its backends are gone), then drops the
    /// ephemeral database. `WITH (FORCE)` is belt-and-braces: it severs any
    /// straggling session instead of failing the DROP — critical because a
    /// failing Drop would otherwise mask the actual test result.
    ///
    /// The whole cleanup future (pool close + DROP) is bounded by
    /// [`DROP_CLEANUP_TIMEOUT`]: a stuck server must cost the run a named
    /// leftover, not a hung process. A leaked database is NOT swept
    /// automatically by later runs — only a run that reuses this exact name
    /// (same pid + slug) would reclaim it — so every failure path below names
    /// the exact database for manual cleanup.
    fn drop(&mut self) {
        let drop_sql = format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.db_name);
        // The timeout future must be CONSTRUCTED inside this async block:
        // `tokio::time::timeout` grabs the reactor handle at construction
        // time, and `block_on`'s argument is evaluated outside the runtime.
        let cleanup = self.rt.block_on(async {
            tokio::time::timeout(DROP_CLEANUP_TIMEOUT, async {
                self.pool.close().await;
                sqlx::raw_sql(&drop_sql).execute(&self.admin_pool).await
            })
            .await
        });
        match cleanup {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => fail_drop_cleanup(&format!(
                "FAILED to drop ephemeral database {}: {e} — leaked on the \
                 scratch server; drop it manually: {drop_sql}",
                self.db_name
            )),
            Err(_elapsed) => fail_drop_cleanup(&format!(
                "drop cleanup for ephemeral database {} timed out after {}s — \
                 leaked on the scratch server; drop it manually: {drop_sql}",
                self.db_name,
                DROP_CLEANUP_TIMEOUT.as_secs()
            )),
        }
    }
}

/// Reports a failed `Drop` cleanup without ever double-panicking: a panic
/// while another panic unwinds aborts the process and hides the real test
/// failure, so that case only `eprintln!`s. Both messages must name the exact
/// database — there is no later run that sweeps it.
fn fail_drop_cleanup(msg: &str) {
    if std::thread::panicking() {
        eprintln!("D2c harness: {msg}");
    } else {
        panic!("D2c harness: {msg}");
    }
}

// ---------------------------------------------------------------------------
// Admin-suite machinery (extracted from `d2c_admin_auth.rs`, D2c step 4)
//
// Shared by the DB-backed admin suites so both boot, route and drive the
// handlers through ONE definition. Moved verbatim where possible; the doc
// comments kept their original claims.
// ---------------------------------------------------------------------------

/// The bootstrapped admin (fake credentials only — never a real secret).
pub const ADMIN_USERNAME: &str = "admin-test";
pub const ADMIN_PASSWORD: &str = "test-admin-password-fake-0001";

/// Fake HMAC key for session cookies. Session tokens are signed, not
/// encrypted, so this is a test-only secret of the real minimum shape: at
/// least 32 bytes, exactly what `config::session_secret()` demands at boot.
pub const SESSION_SECRET: &[u8] = b"d2c-admin-auth-test-secret-0123456789abcdef";

/// Upper bound on a response body read. Sized for the largest response the
/// suites assert on: `GET /api/catalogue` carries all 8 SKUs with their
/// per-locale content PLUS the R1 media/spec fields (that aggregate is past
/// the old 16 KiB). Still an upper bound: a runaway body fails the read
/// loudly (LengthLimitError) instead of ballooning a test process.
pub const BODY_LIMIT: usize = 64 * 1024;

/// A product id that cannot exist: the seed allocates ids 1..8 and each test
/// starts from that fresh state.
pub const MISSING_PRODUCT_ID: i64 = 999_999;

/// Exact 404 body the product-bearing handlers guarantee (shared: the admin
/// suites both assert the missing-product contract).
pub const PRODUCT_NOT_FOUND_404: &str = r#"{"error":"product not found"}"#;

/// `products` row shape of the direct-SQL witness queries: (id, slug, brand,
/// model, price_cents, family, art_variant, art_hue, active, stock_total,
/// stock_reserved). `created_at` is a clock value — never exactly checkable
/// against a literal — so the row witness excludes it; the auth round-trip
/// test cross-checks the response's `created_at` against the row's Postgres
/// timestamp separately (`product_created_at`, parsed + within 1s).
pub type ProductRow = (
    i64,
    String,
    String,
    String,
    i64,
    String,
    String,
    i32,
    bool,
    i32,
    i32,
);

/// `audit_log` row shape: (actor, action, entity, entity_id, detail).
pub type AuditRow = (String, String, String, Option<String>, Value);

/// Builds the boot-equivalent admin stack on the scratch database: the
/// `AppState` `main` builds (pool, session key, login gate sized by
/// `admin::LOGIN_MAX_CONCURRENT`, `admin_ready` starting false) followed by
/// the boot bootstrap that upserts the admin credential and marks auth ready.
///
/// The bootstrap runs through `admin::bootstrap_with` with an injected
/// `Rotation::Requested` — the same code path as `admin::bootstrap_admin`
/// (argon2id hash + `admin_users` upsert + the ready flag), minus the process
/// env read: `bootstrap_admin` parses `ADMIN_USERNAME`/`ADMIN_PASSWORD` from
/// the environment, and mutating the process env from parallel test threads
/// would race (exactly why the D2b tests use the same injected seam).
pub fn boot_admin(db: &ScratchDb) -> AppState {
    // Live wiring of the pure guard: READ-ONLY env access (mutating the
    // process env from parallel test threads would race), with unset-or-empty
    // both counting as clean.
    assert_no_revalidate_webhook_env(|name| std::env::var(name).ok().filter(|v| !v.is_empty()));
    let state = AppState {
        pool: Some(db.pool().clone()),
        session_key: Some(SessionKey::new(SESSION_SECRET.to_vec())),
        login_gate: Arc::new(Semaphore::new(admin::LOGIN_MAX_CONCURRENT)),
        admin_ready: Arc::new(AtomicBool::new(false)),
    };
    db.block_on(admin::bootstrap_with(
        state.pool.as_ref(),
        state.session_key.as_ref(),
        Rotation::Requested {
            username: ADMIN_USERNAME.to_string(),
            password: ADMIN_PASSWORD.to_string(),
        },
        state.admin_ready.clone(),
    ));
    assert!(
        state.admin_ready.load(Ordering::Acquire),
        "boot-equivalent bootstrap must mark admin auth ready (the upsert \
         succeeded; check the scratch DB logs on failure)"
    );
    state
}

/// The handlers under test fire the commitment-5b revalidation webhook
/// (`tokio::spawn`ed) after every COMMITTED mutation, reading
/// `REVALIDATE_URL`/`REVALIDATE_SECRET` from the ambient environment. A set
/// knob would open real network connections from the admin suites, and tests
/// must never call live services — so refuse the run loudly instead of
/// silently poking a real web tier.
///
/// Pure over `lookup`, mirroring the style of
/// [`assert_ambient_pg_env_clean`]: the mock-env policy regressions in the
/// suite files drive it WITHOUT touching the process environment (mutating
/// that would race the parallel test threads), while [`boot_admin`] wires it
/// to the live environment with one closure.
///
/// The panic names ONLY the variable — never its value. `REVALIDATE_SECRET`
/// is a live credential, so interpolating it into a panic would print it into
/// test output; `REVALIDATE_URL` gets the same name-only treatment so one
/// uniform rule covers both knobs.
pub fn assert_no_revalidate_webhook_env(lookup: impl Fn(&str) -> Option<String>) {
    for name in ["REVALIDATE_URL", "REVALIDATE_SECRET"] {
        if lookup(name).is_some() {
            panic!(
                "ambient {name} is set (value withheld on purpose: a \
                 revalidation knob can be a live secret and must never reach \
                 test output): the admin mutations would fire the \
                 revalidation webhook at a real network target from this test \
                 run; unset it before running the D2c admin-auth suite"
            );
        }
    }
}

/// The ambient-`LOCALES` guard (D2c step 4): the locale-gating assertions of
/// the admin-mutation suite depend on the configured locale pair being the
/// code DEFAULT (`en`, `zh-HK` — `config::DEFAULT_LOCALES`), because they pin
/// both a member (`en`/`zh-HK` accepted) and a non-member (`fr` rejected). A
/// set ambient `LOCALES` would silently change which tags are members and
/// make those assertions non-deterministic — so a set value refuses the run
/// loudly instead of asserting against whatever the machine happens to
/// configure.
///
/// Same refusal pattern as [`assert_no_revalidate_webhook_env`]: pure over
/// `lookup` (the mock-env regression tests drive it without touching the
/// process environment), unset-or-empty both count as clean (mirroring
/// `config::locales()`, which treats an unparseable/empty value as the
/// default), and the panic names ONLY the variable — never its value — so one
/// secret-hygiene rule covers every ambient knob.
pub fn assert_no_locales_env(lookup: impl Fn(&str) -> Option<String>) {
    const NAME: &str = "LOCALES";
    if lookup(NAME).is_some() {
        panic!(
            "ambient {NAME} is set (value withheld on purpose: ambient \
             configuration must never reach test output): the admin-mutation \
             suite's locale-gating assertions depend on the configured \
             default locale pair (en, zh-HK) and would become \
             non-deterministic under a custom pair; unset it before running \
             the D2c admin-mutation suite"
        );
    }
}

/// The admin routes under test, mounted from the LIB handlers at the same
/// paths the binary's `fn app()` uses (`/api/admin/login` public, everything
/// else behind `admin::require_admin`), plus the PUBLIC `GET /api/settings`
/// read the settings round-trip tests assert against. The bin's `fn app()`
/// lives in a bin target and is not reachable from integration tests, so the
/// FULL route wiring (CORS origin lock, trace layer, storefront routes) stays
/// pinned by the bin tests in `api/src/main.rs`; this mounts only the slice
/// the DB-backed behavior needs, at the real paths.
pub fn admin_router(state: AppState) -> Router {
    admin_router_with_observer(state, None)
}

/// The same admin routes with an optional counting OBSERVER middleware
/// mounted BETWEEN the `require_admin` auth middleware and the handlers.
///
/// WHY THE OBSERVER EXISTS: the handlers have their own `actor()` check that
/// answers the identical 401 when no `CurrentUser` extension is present (see
/// `admin_crud::create_product`), so a status code alone cannot prove the
/// AUTH MIDDLEWARE (rather than the handler) refused a request — and a
/// middleware regression that forwarded unauthenticated requests to the
/// handler would still look green. Counting handler-stack entries makes the
/// middleware boundary observable; the positive control in
/// `unauthenticated_and_csrfless_mutations_are_401_and_403` then proves the
/// counter itself observes the handler path.
///
/// LAYER ORDER (axum 0.8): the LAST `.layer()` added is the OUTERMOST. So the
/// routes are mounted first, the observer is layered next (inner), and
/// `require_admin` is layered last (outer) — a request is authenticated FIRST
/// and only reaches the observer (and then the handlers) when `require_admin`
/// calls `next`. If the order were flipped the observer would be outermost
/// and the unauthenticated request would already count a hit — which is
/// exactly what the test's `hits == 0` assertions reject.
///
/// The mounted admin surface is the COMPLETE one from `main.rs`'s `fn app()`
/// (step 4 extension: the mutation suite drives every CRUD path; the step-3
/// tests exercise the same paths they always did and are unaffected).
pub fn admin_router_with_observer(
    state: AppState,
    observer_hits: Option<Arc<AtomicUsize>>,
) -> Router {
    let routes: Router<AppState> = Router::new()
        .route("/whoami", get(admin::whoami))
        .route("/logout", post(admin::logout))
        .route(
            "/products",
            get(admin_crud::list_products).post(admin_crud::create_product),
        )
        .route(
            "/products/{id}",
            get(admin_crud::get_product_full).put(admin_crud::update_product),
        )
        .route("/products/{id}/full", put(admin_crud::update_product_full))
        .route("/product-content", put(admin_crud::upsert_product_content))
        .route(
            "/reference-prices/{product_id}",
            put(admin_crud::upsert_reference_price).delete(admin_crud::delete_reference_price),
        )
        .route("/pieces", post(admin_crud::create_piece))
        .route(
            "/pieces/{piece_no}",
            patch(admin_crud::reassign_piece).delete(admin_crud::delete_piece),
        )
        .route("/site-content", put(admin_crud::upsert_site_content))
        .route(
            "/site-content/{key}/{locale}",
            delete(admin_crud::delete_site_content),
        )
        .route(
            "/settings/{key}",
            put(admin_crud::upsert_setting).delete(admin_crud::delete_setting),
        )
        // Money-path admin surface (R5a): order inspection + the three
        // stock/state operations. Mounted here at the SAME paths the binary's
        // `fn app()` uses so the suites can never drift from the served
        // surface.
        .route("/orders", get(orders::list_orders))
        .route("/orders/{id}/confirm-fps", post(orders::confirm_fps_order))
        .route("/orders/{id}/release", post(orders::release_order))
        .route(
            "/orders/{id}/quarantine-return",
            post(orders::quarantine_return_order),
        )
        .route("/audit-log", get(admin_crud::list_audit_log));
    let routes = match observer_hits {
        Some(hits) => routes.layer(middleware::from_fn(
            move |req: Request<Body>, next: middleware::Next| {
                let hits = Arc::clone(&hits);
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    next.run(req).await
                }
            },
        )),
        None => routes,
    };
    let admin_protected: Router<AppState> = routes.layer(middleware::from_fn_with_state(
        state.clone(),
        admin::require_admin,
    ));
    Router::new()
        .route("/api/admin/login", post(admin::login))
        .route("/api/settings", get(content::get_settings))
        .nest("/api/admin", admin_protected)
        .with_state(state)
}

/// One admin-route request with the CSRF custom header the mutations require
/// (`X-Requested-With`, same value the bin tests use) and, optionally, the
/// session cookie.
pub fn admin_request(method: Method, uri: &str, cookie: Option<&str>, body: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-requested-with", "test-client");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    builder
        .body(Body::from(body.to_string()))
        .expect("test request must build")
}

/// The session cookie header, minted through the SAME signer the login
/// handler uses (`admin::session_cookie_value`): the cookie's `sub`/TTL/HMAC
/// semantics are D2b-unit-pinned and the live login round-trip is pinned by
/// the login tests, so the mutation tests mint directly and keep the suite's
/// argon2 budget at one verify per login test.
pub fn minted_session_cookie(state: &AppState) -> String {
    let key = state
        .session_key
        .as_ref()
        .expect("test state always carries a session key");
    let token = admin::session_cookie_value(ADMIN_USERNAME, key.as_bytes(), Utc::now().timestamp());
    format!("{}={token}", admin::SESSION_COOKIE_NAME)
}

/// Drives one request through the router on the harness runtime and splits
/// the answer into (status, headers, body) so every part can be asserted
/// exactly.
pub async fn send(router: &Router, req: Request<Body>) -> (StatusCode, HeaderMap, String) {
    let response = router
        .clone()
        .oneshot(req)
        .await
        .expect("router must answer");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), BODY_LIMIT)
        .await
        .expect("response body must be readable");
    let body = String::from_utf8(bytes.to_vec()).expect("response body must be UTF-8");
    (status, headers, body)
}

/// One `products` row by slug, read with direct SQL — the DB is the witness
/// for what a mutation wrote, never the handler's own response.
pub fn product_row(db: &ScratchDb, slug: &str) -> Option<ProductRow> {
    db.block_on(async {
        sqlx::query_as::<_, ProductRow>(
            "SELECT id, slug, brand, model, price_cents, family, art_variant, \
                 art_hue, active, stock_total, stock_reserved \
             FROM products WHERE slug = $1",
        )
        .bind(slug)
        .fetch_optional(db.pool())
        .await
        .unwrap_or_else(|e| panic!("products lookup failed for slug {slug}: {e}"))
    })
}

/// ALL `audit_log` rows in id order (direct SQL).
pub fn audit_rows(db: &ScratchDb) -> Vec<AuditRow> {
    db.block_on(async {
        sqlx::query_as::<_, AuditRow>(
            "SELECT actor, action, entity, entity_id, detail FROM audit_log ORDER BY id",
        )
        .fetch_all(db.pool())
        .await
        .expect("audit_log lookup failed")
    })
}

/// Full `pieces` numbering snapshot: (piece_no, product_id) in piece order.
pub fn pieces_snapshot(db: &ScratchDb) -> Vec<(i32, i64)> {
    db.block_on(async {
        sqlx::query_as::<_, (i32, i64)>("SELECT piece_no, product_id FROM pieces ORDER BY piece_no")
            .fetch_all(db.pool())
            .await
            .expect("pieces snapshot failed")
    })
}

/// `SELECT COUNT(*)` over `products`, optionally narrowed to one slug
/// (constants and bound values only — identifiers cannot be bound).
pub fn product_count(db: &ScratchDb, slug: Option<&str>) -> i64 {
    db.block_on(async {
        let count = match slug {
            Some(slug) => {
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM products WHERE slug = $1")
                    .bind(slug)
                    .fetch_one(db.pool())
                    .await
            }
            None => {
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM products")
                    .fetch_one(db.pool())
                    .await
            }
        };
        count.unwrap_or_else(|e| panic!("products count failed: {e}"))
    })
}

/// The id of the first seeded product (the seed allocates ids 1..8).
pub fn first_product_id(db: &ScratchDb) -> i64 {
    db.block_on(async {
        sqlx::query_scalar::<_, i64>("SELECT id FROM products ORDER BY id LIMIT 1")
            .fetch_one(db.pool())
            .await
            .expect("first product lookup failed")
    })
}

/// The product's `created_at`, read with direct SQL — the DB timestamp is the
/// witness the auth round-trip's response `created_at` is cross-checked
/// against.
pub fn product_created_at(db: &ScratchDb, slug: &str) -> DateTime<Utc> {
    db.block_on(async {
        sqlx::query_scalar::<_, DateTime<Utc>>("SELECT created_at FROM products WHERE slug = $1")
            .bind(slug)
            .fetch_optional(db.pool())
            .await
            .unwrap_or_else(|e| panic!("products created_at lookup failed for slug {slug}: {e}"))
            .expect("created product row must exist in Postgres")
    })
}
