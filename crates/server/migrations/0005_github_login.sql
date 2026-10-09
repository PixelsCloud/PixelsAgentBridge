ALTER TABLE users ALTER COLUMN password_hash DROP NOT NULL;

CREATE TABLE external_identities (
    provider text NOT NULL CHECK (provider = 'github'),
    provider_user_id text NOT NULL,
    user_id uuid NOT NULL REFERENCES users(id),
    provider_login text NOT NULL,
    avatar_url text,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (provider, provider_user_id),
    UNIQUE (provider, user_id)
);

-- Short-lived transactions, never GitHub access tokens or long-lived PAB tokens.
CREATE TABLE github_authorizations (
    state_hash text PRIMARY KEY,
    cookie_hash text,
    launch_hash text UNIQUE,
    pkce_verifier text NOT NULL,
    session_hash text REFERENCES web_sessions(token_hash) ON DELETE CASCADE,
    user_id uuid REFERENCES users(id),
    auth_revision bigint,
    redirect_uri text,
    client_state text,
    proof_hash text,
    expires_at timestamptz NOT NULL DEFAULT now() + interval '10 minutes',
    CHECK ((user_id IS NULL) = (session_hash IS NULL)),
    CHECK ((redirect_uri IS NULL) = (proof_hash IS NULL)),
    CHECK ((redirect_uri IS NULL) = (client_state IS NULL))
);
CREATE INDEX github_authorizations_expiry ON github_authorizations(expires_at);

CREATE TABLE github_redemptions (
    code_hash text PRIMARY KEY,
    proof_hash text NOT NULL,
    user_id uuid NOT NULL REFERENCES users(id),
    auth_revision bigint NOT NULL,
    expires_at timestamptz NOT NULL DEFAULT now() + interval '1 minute'
);
CREATE INDEX github_redemptions_expiry ON github_redemptions(expires_at);
