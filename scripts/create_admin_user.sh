#!/usr/bin/env bash
# Creates (or promotes) an admin account using this project's own DB config and
# password hashing, so anyone who clones the repo can quickly get an admin login.
set -euo pipefail

cd "$(dirname "$0")/.."

set -a
[[ -f .env ]] && source .env
app_env="${APP_ENV:-development}"
[[ -f ".env.${app_env}" ]] && source ".env.${app_env}"
set +a

if [[ -z "${DATABASE_URL:-}" ]]; then
  echo "DATABASE_URL is not set. Copy .env.example to .env (and .env.development.example to .env.development) first." >&2
  exit 1
fi

username="${1:-}"
if [[ -z "$username" ]]; then
  read -rp "Username: " username
fi

if [[ -n "${2:-}" ]]; then
  password="$2"
else
  read -rsp "Password: " password
  echo
  read -rsp "Confirm password: " password_confirm
  echo
  if [[ "$password" != "$password_confirm" ]]; then
    echo "Passwords do not match." >&2
    exit 1
  fi
fi

ADMIN_NEW_USERNAME="$username" ADMIN_NEW_PASSWORD="$password" cargo run --quiet --bin create_admin
