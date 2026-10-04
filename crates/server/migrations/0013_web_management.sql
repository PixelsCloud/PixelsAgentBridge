-- Browser management is independent of remote command/task storage.
ALTER TABLE users ADD COLUMN server_admin boolean NOT NULL DEFAULT false;

CREATE TABLE web_sessions (
    token_hash text PRIMARY KEY CHECK (length(token_hash) = 64),
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    auth_revision bigint NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    CHECK (expires_at > created_at)
);
CREATE INDEX web_sessions_user ON web_sessions(user_id);
CREATE INDEX web_sessions_expiry ON web_sessions(expires_at);

CREATE TABLE web_admin_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_id uuid REFERENCES users(id) ON DELETE SET NULL,
    action text NOT NULL,
    resource_id uuid,
    created_at timestamptz NOT NULL DEFAULT now()
);
-- Deliberately no arbitrary JSON/body: never persist passwords or task data.

ALTER TABLE devices ADD COLUMN last_online_at timestamptz;
