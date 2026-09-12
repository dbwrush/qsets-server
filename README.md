# qsets-server

Rust/Axum + Askama web server for Nazarene Bible quizzing question pools with tiered access control.

## Stack

- Rust 2021
- Axum 0.7
- Askama templates
- SQLx + PostgreSQL
- Vanilla HTML/CSS/JS frontend

## Features

- Session login with Argon2 password verification
- CSRF token checks on mutating API endpoints
- Login attempt throttling
- Tier-based pool visibility with inherited lower-tier access
- Admin endpoints and UI for pool upload, tier creation, and user creation
- Audit logging for login/logout, generation, pool upload, tier creation, user creation
- Seeded generation mode for deterministic output

## Generation Modes

- `standard`: 20-question set rules:
  - 1 Situation or 1 In-What-Book-and-Chapter
  - 1 Quote, 1 Reference, 1 Verse, 1 Context
  - 4 According-To
  - Remaining as General
  - Chapter usage cap and replacement rounds with final reshuffle
- `all` or specific type name: random sample mode

## Environment

Copy `.env.example` to `.env`, then optionally add an environment-specific override file:

- `APP_ENV` (`development`, `testing`, or any custom profile name)
- `DATABASE_URL`
- `BIND_ADDR`
- `ADMIN_USERNAME`
- `ADMIN_PASSWORD`
- `COOKIE_SECURE`
- `GENERATION_CONCURRENCY`

Runtime loading order:

1. `.env`
2. `.env.<APP_ENV>` (overrides `.env` values)

Shell environment variables always win over file values. This means you can compile once on a dev machine and run the same binary on prod with different database credentials by changing env vars or profile files.

Examples:

- `.env.development.example`
- `.env.testing.example`

Copy these to `.env.development` and `.env.testing` as needed.

## Local Dev/Test Databases

PostgreSQL tools required:

- Arch Linux: `sudo pacman -S --needed postgresql`
- Ubuntu/Debian: `sudo apt-get install postgresql postgresql-client`
- macOS (Homebrew): `brew install postgresql`

Bootstrap helper:

```bash
./scripts/setup_dev_test_databases.sh
```

SQL equivalent:

```bash
psql -U postgres -d postgres -f scripts/create_dev_test_databases.sql
```

## Run

```bash
cargo run
```

Server starts at `http://127.0.0.1:3000` by default.

## Deployment Handoff

This repository is intended to be portable. It does not assume a particular domain,
cloud provider, Linux distribution, reverse proxy, or PostgreSQL host. The eventual
operator can choose those pieces and run the compiled binary or build their own
container/package around it.

### Application prerequisites

- Rust toolchain compatible with the current stable release
- PostgreSQL 14 or newer (the exact supported version should be confirmed by the operator)
- A process supervisor selected by the operator
- TLS termination at the chosen reverse proxy when exposed outside localhost

### First-time setup

1. Copy `.env.example` to `.env`, or copy the profile example matching `APP_ENV`.
2. Set a strong `DATABASE_URL` and `ADMIN_PASSWORD`; do not use the example password.
3. Create the target PostgreSQL database and ensure the configured role can run migrations.
4. Start the application. It applies the SQLx migrations automatically on startup.
5. The configured admin account is created on first startup if that username does not exist.
6. Sign in, create the desired tiers, and upload the real question pools through `/admin`.

The application binds to `127.0.0.1:3000` by default. A reverse proxy can forward the
chosen public hostname to that address. Set `COOKIE_SECURE=true` whenever HTTPS is in
use. Keep the application and PostgreSQL credentials outside source control.

### Operational checklist

Before handing the service to users, verify:

- `GET /api/health` responds successfully through the chosen local/proxy path.
- Login, logout, session expiry, and admin authorization work.
- Login and logout requests include the CSRF token issued by the page.
- Public and restricted tier visibility match the intended policy.
- Pool upload validation reports the expected valid and skipped row counts.
- Oversized pool uploads are rejected by the application before processing.
- Generated RTF and QSET files work with their downstream consumers.
- PostgreSQL backups and restore procedures have been tested.
- Logs are collected by the chosen process supervisor.
- The reverse proxy passes the real client address only when it is trusted.

The application currently has strong unit/integration coverage for CSV parsing and
generation, but not yet for HTTP routes, PostgreSQL behavior, or browser workflows.
Those checks should be completed in a staging environment before public release.

## Single-Server Runtime Notes

The application can run as one process with PostgreSQL on the same machine. Axum and
Tokio handle concurrent network requests, while question generation is moved to the
runtime's blocking worker pool so CPU-heavy generation does not occupy the async I/O
workers. A semaphore limits concurrent generation tasks to the number of logical CPUs
by default; set `GENERATION_CONCURRENCY` lower if the same machine is doing other work.
Parsed question pools are cached in process for faster repeated generation; up to 16
pools are retained and the cache is rebuilt after a restart.

## Test

```bash
APP_ENV=testing cargo test
```

or:

```bash
cargo test
```

Parity tests are in `tests/generator_parity.rs`.

### Local load testing

With PostgreSQL running and a test server listening locally, the standard-library
harness can exercise concurrent generation without additional packages:

```bash
python3 scripts/load_test.py --base-url http://127.0.0.1:3010 --users 100 --requests 5
```

The harness reports throughput, median/p95/max latency, and failures. Use a realistic
pool when evaluating deployment capacity; extremely large pools can make generation
CPU-bound even though normal 1,000--2,000-question pools remain small.
