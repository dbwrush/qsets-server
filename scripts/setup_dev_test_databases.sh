#!/usr/bin/env bash
set -euo pipefail

PSQL_BIN="${PSQL_BIN:-psql}"
PGUSER_OPT="${PGUSER:-postgres}"

if ! command -v "$PSQL_BIN" >/dev/null 2>&1; then
  echo "psql not found. Install PostgreSQL client tools first."
  exit 1
fi

create_db_if_missing() {
  local db_name="$1"
  local exists
  exists="$($PSQL_BIN -U "$PGUSER_OPT" -tAc "SELECT 1 FROM pg_database WHERE datname='${db_name}'" postgres || true)"
  if [[ "$exists" == "1" ]]; then
    echo "Database ${db_name} already exists"
  else
    echo "Creating database ${db_name}"
    "$PSQL_BIN" -U "$PGUSER_OPT" -c "CREATE DATABASE ${db_name};" postgres
  fi
}

create_db_if_missing qsets_dev
create_db_if_missing qsets_test

echo "Done. Dev/Test databases are ready."
