-- Existing accounts have no email until the owner (or an admin) adds one.
ALTER TABLE users
    ADD COLUMN IF NOT EXISTS email TEXT,
    ADD COLUMN IF NOT EXISTS email_verified_at TIMESTAMPTZ;

-- Only verified addresses are unique, so nobody can lock an address out by registering it
-- without being able to read its mail. Login and password reset only trust verified addresses.
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_verified_email
    ON users (lower(email)) WHERE email_verified_at IS NOT NULL;

-- One-time links mailed to users. Only a hash of each token is stored, so a database leak does
-- not hand out working links. `email` is the address the link proves ownership of.
CREATE TABLE IF NOT EXISTS email_tokens (
    token_hash TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    purpose TEXT NOT NULL CHECK (purpose IN ('verify_email', 'reset_password')),
    email TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_email_tokens_user ON email_tokens (user_id, purpose);
