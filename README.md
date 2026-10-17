# qsets-server

Rust/Axum + Askama web server for Nazarene Bible quizzing question pools with tiered access control.

## Stack

- Rust 2021
- Axum 0.7
- Askama templates
- SQLx + PostgreSQL
- Vanilla HTML/CSS/JS frontend

## Features

- Session login with Argon2 password verification; unknown usernames take as long to reject
  as wrong passwords, so timing does not reveal which accounts exist
- CSRF token checks on every mutating API endpoint, applied as middleware
- Login throttling per account-and-client, per client (stops password spraying) and per
  account (slows distributed guessing; anyone can spend this limit to lock an account out
  for 15 minutes, which is why it is set much higher than the others)
- Self-service password change at `/account`; any password change signs the user out of
  their other sessions
- Self-service accounts at `/register` (username, email, password). New accounts get public
  access until an admin raises their tier, and must confirm their email address before signing in
- Sign in with a username or a confirmed email address
- Forgot-password links by email (`/forgot-password`); a reset signs the account out everywhere.
  Verification and reset requests answer identically whether or not the address has an account,
  are rate limited per client and per address, and use single-use tokens stored only as hashes
- Content-Security-Policy and related security headers on every response
- Tier-based pool visibility with inherited lower-tier access
- Admin endpoints and UI for pool upload, replacement and deletion, tier creation, and user creation
- Audit logging for login/logout, password changes, generation, pool, tier and user changes,
  with an admin viewer and configurable retention
- Seeded generation mode for deterministic output

## Generation Modes

- `standard`: 20-question set rules:
  - 1 Situation or 1 In-What-Book-and-Chapter
  - 1 Quote, 1 Reference, 1 Verse, 1 Context
  - 4 According-To
  - Remaining as General
  - Chapter usage cap and replacement rounds with final reshuffle
- `all` or specific type name: random sample mode
- `custom`: choose a nonnegative whole-number count for each of the eight question
  types in step 3. Zero excludes a type; the total can be any positive number.
  Each set has exactly the requested distribution, without repeating a pool entry.
  The chapter cap is `max(2, ceil(total / chapter_count))`, where `chapter_count`
  counts all distinct chapters available in the filtered pool, as in standard mode.
  Missing questions or an impossible chapter distribution produce an error rather
  than a shortened or relaxed set. Seeds, multiple sets, and exports work as usual.
  Situation Questions remains the default mode on page load and Start over.
  Custom controls show requested versus available counts for the selected passages,
  highlight shortages, and disable Generate sets until the server confirms the
  counts and chapter distribution are feasible. Generation failures appear in a
  dismissible pop-up.

Standard generation never relaxes these rules. If the selected books/chapters lack
enough questions of a required type, generation reports the required and available
counts instead of returning an empty set. Failures caused by the chapter usage cap
also report the affected type and limit. The API returns HTTP 400 with an `error`
message, which the generator form displays in a pop-up; multi-set requests fail as a whole.

For Custom API requests, send `"question_type": "custom"` and a `custom_counts`
object using the CSV type names as keys, for example
`{"Situation": 1, "According-To": 4, "General": 5}`. Omitted types default to zero;
unknown keys, negative counts, and fractional counts are rejected. The total is
derived from `custom_counts`, not the legacy `count` field. Other modes must omit
`custom_counts`.

`POST /api/generate/validate` accepts the same Custom request fields, requires a
nonempty `books` selection, and returns `valid`, `error` (null when valid), and
`available` counts for all eight types. It checks the same chapter constraints
without recording a generation event or returning question sets.

## Study mode

Open **Study** in the shared header (`/study`), choose a pool, passages, question
types, and session length (0 means all matching questions). Questions are shuffled
without repeating a pool entry; a request larger than the available pool uses all
matching questions. Study does not use standard-set distributions or chapter caps.

Each question has 20 seconds from display to complete and submit an answer.
The clock continues while the tab is inactive. Expiry does not prevent submission:
correctness and submitting within 20 seconds are tracked separately. Normalized
exact matches ignore case, punctuation, and extra whitespace. Other answers require
self-assessment against the expected answer. Revealing/skipping is not correct.
The summary separates correct/on-time, correct/late, incorrect, skipped, and
self-assessed correct answers; missed and late questions can be reviewed.

Study state exists only in browser memory: no database writes, local/session storage,
cookies containing study progress, or study audit events. Refresh/navigation discards
the session. The access-controlled `POST /api/study/questions` endpoint returns a
randomized question batch with `Cache-Control: no-store`. Answers are loaded into
browser memory with the batch and hidden until submission/reveal; this is practice,
not a cheat-resistant examination. Answers, timing, and results are never submitted
to the server. Existing shared/private pool permissions apply.

JavaScript study logic tests: `node --test tests/study-session.test.mjs`.

## Environment

Copy `.env.example` to `.env`, then optionally add an environment-specific override file:

- `APP_ENV` (`development`, `testing`, or any custom profile name)
- `DATABASE_URL`
- `BIND_ADDR`
- `ADMIN_USERNAME`
- `ADMIN_PASSWORD`
- `COOKIE_SECURE`
- `GENERATION_CONCURRENCY`
- `TRUST_PROXY` (default `false`): set to `true` only when a reverse proxy sets
  `X-Forwarded-For`. When false the header is ignored, because clients could otherwise
  forge it to dodge login throttling and falsify audit-log IPs.
- `AUDIT_RETENTION_DAYS` (default `365`): audit entries older than this are deleted by an
  hourly sweep. `0` keeps them forever.
- `PUBLIC_BASE_URL`: origin used in emailed links, e.g. `https://qsets.example.com`. Required
  outside `development`; it is configured rather than taken from the request so a forged `Host`
  header cannot redirect reset links.
- `SMTP_HOST`, `SMTP_FROM`, and optionally `SMTP_PORT`, `SMTP_USERNAME`, `SMTP_PASSWORD`,
  `SMTP_TLS` (`starttls` default, `tls`, or `none`): outgoing email. Without `SMTP_HOST`, emails
  are only written to the server log, which suits development but means nobody can confirm an
  address.
- `ALLOW_REGISTRATION` (default `true`): set `false` to close self-service sign-up.
- `STATIC_DIR` (default `static`): location of the CSS/JS assets. Relative paths resolve
  against the working directory; templates are compiled into the binary.

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

### Create an admin account

```bash
./scripts/create_admin_user.sh <username> [password]
```

Reads `DATABASE_URL` from `.env`/`.env.<APP_ENV>`, prompts for a password if one isn't
passed as an argument, and hashes it the same way the app does. Safe to re-run: an
existing username is promoted to admin with the new password instead of failing.

Passwords must be at least 8 characters, whether set by this script, the admin page,
or `ADMIN_PASSWORD` outside the `development` profile.

### Accounts and email

Accounts created before email support have no address. Their owners can add one on `/account`
(it takes effect once its link is opened), or an admin can set one on the Users tab; an address an
admin sets counts as confirmed. Accounts without a confirmed address cannot use password reset.

Only confirmed addresses are unique and usable for sign-in or reset, so registering someone
else's address does not block them. Sign-ups that never confirm are deleted after 3 days.
Usernames chosen at registration cannot contain `@`, so a username is never mistaken for an email.

### Tiers

Anonymous visitors read at rank 0, so the tier with rank 0 is the public tier. It can be
renamed but not re-ranked or deleted. Shared tiers cannot have a negative rank.

The built-in `private` tier is a pool-only access mode (reserved rank -1), not a
level in the shared hierarchy. Only the uploader and admins can list its pools,
read their books, validate custom configurations, or generate questions from them.
Higher user tiers do not grant private access. Private cannot be renamed, re-ranked,
deleted, or assigned to a user. Migration `0005_private_pool_tier.sql` adds it;
the name `private` and rank -1 must not already be used by custom tiers.

### Uploading and managing pools

Users with **Can upload pools** permission (and admins) have a **My pools** link
in their account menu at `/pools`. They can upload a CSV, replace its questions,
or delete their own pools. `/admin` remains available for admins and as a
compatibility URL for uploaders; non-admins never see user, tier, or audit tools.
Shared uploads are limited to the uploader's tier or lower; choosing **private**
keeps the pool visible only to its uploader and admins. The management table
shows who uploaded each pool, including private pools viewed by admins.

Revoking upload permission prevents new uploads and replacement/deletion, but an
owner can still read and generate from their private pools. Pools whose uploader
has been deleted remain visible only to admins and show an unknown/deleted owner.

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
use, and `TRUST_PROXY=true` when the proxy sets `X-Forwarded-For`. The proxy must replace
or append to that header rather than pass a client-supplied value through. The server
uses the right-most entry, so exactly one trusted proxy is assumed. Run the binary from
the repository root, or set `STATIC_DIR` to the absolute path of `static/`. Keep the
application and PostgreSQL credentials outside source control.

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

Automated tests cover CSV parsing, generation, and the HTTP routes against PostgreSQL
(tier access, upload permissions, CSRF, login throttling, session revocation). Browser
workflows and the downstream use of generated files are not automated yet and should be
checked in a staging environment before public release.

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

HTTP route tests in `tests/http_routes.rs` need a running PostgreSQL. Each test creates
and drops its own throwaway database on the server in `DATABASE_URL` (read from the
environment or `.env`), so that role needs `CREATEDB`. Your app data is not touched,
but sqlx keeps a small `_sqlx_test` bookkeeping schema in the database named in the URL.

If they fail with `template database "template1" has a collation version mismatch`,
the OS C library was upgraded after the cluster was created. Fix the cluster
(`ALTER DATABASE template1 REFRESH COLLATION VERSION`, after reindexing as the hint
describes), or point `DATABASE_URL` at another server.

### Local load testing

With PostgreSQL running and a test server listening locally, the standard-library
harness can exercise concurrent generation without additional packages:

```bash
python3 scripts/load_test.py --base-url http://127.0.0.1:3010 --users 100 --requests 5
```

The harness reports throughput, median/p95/max latency, and failures. Use a realistic
pool when evaluating deployment capacity; extremely large pools can make generation
CPU-bound even though normal 1,000--2,000-question pools remain small.
