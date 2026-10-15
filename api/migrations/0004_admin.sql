-- 0004_admin.sql — admin console auth (ADMIN-01, commitment 2).
--
-- The admin users table. Passwords are argon2id PHC strings, never
-- plaintext; the initial admin user is NOT seeded here (no secrets in the
-- repo or the seed): the server upserts it at boot from ADMIN_USERNAME /
-- ADMIN_PASSWORD when both are set (see api/src/main.rs).
--
-- Sessions are stateless HMAC-signed cookies (SESSION_SECRET, env-only):
-- no session table to purge; a token carries its own expiry and the
-- signature, so a stolen cookie cannot be forged or extended, only replayed
-- until it expires.

CREATE TABLE admin_users (
    id BIGSERIAL PRIMARY KEY,
    -- Cookie-safe charset: tokens are hex-encoded anyway, but keep usernames
    -- simple so logs and audit rows stay unambiguous.
    username TEXT UNIQUE NOT NULL
        CHECK (username ~ '^[A-Za-z0-9_-]{1,64}$'),
    -- argon2id PHC string ($argon2id$...). Never plaintext.
    password_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);