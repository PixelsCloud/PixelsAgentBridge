-- Account display/management associations are independent of device authorization.
CREATE TABLE device_accounts (
    device_id uuid PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
    user_id uuid REFERENCES users(id) ON DELETE RESTRICT,
    revision bigint NOT NULL DEFAULT 0 CHECK (revision >= 0),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX device_accounts_user ON device_accounts(user_id) WHERE user_id IS NOT NULL;

CREATE TABLE device_account_challenges (
    id uuid PRIMARY KEY,
    device_id uuid NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    session_hash text NOT NULL REFERENCES web_sessions(token_hash) ON DELETE CASCADE,
    payload jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    applied_revision bigint
);
CREATE INDEX device_account_challenges_expiry ON device_account_challenges(expires_at);
