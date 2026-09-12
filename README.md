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
