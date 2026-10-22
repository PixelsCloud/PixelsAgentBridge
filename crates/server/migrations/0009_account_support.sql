-- Preserve existing token hashes and permanent sessions; legacy metadata is unknown.
ALTER TABLE web_sessions
    ADD COLUMN id uuid NOT NULL DEFAULT gen_random_uuid(),
    ADD COLUMN channel text NOT NULL DEFAULT 'unknown' CHECK (channel IN ('unknown','browser','native')),
    ADD COLUMN last_active_at timestamptz;
CREATE UNIQUE INDEX web_sessions_public_id ON web_sessions(id);
CREATE INDEX web_sessions_account_created ON web_sessions(user_id,created_at,id);

-- Results only: never persist passwords, password fingerprints or session hashes here.
CREATE TABLE account_support_operations (
    id uuid PRIMARY KEY,
    actor_id uuid NOT NULL REFERENCES users(id),
    target_id uuid NOT NULL REFERENCES users(id),
    kind text NOT NULL CHECK (kind IN ('password_reset','session_revoke')),
    scope text CHECK (scope IN ('all','others','session')),
    session_id uuid,
    revoke_sessions boolean NOT NULL,
    sessions_revoked bigint NOT NULL CHECK (sessions_revoked >= 0),
    completed_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK ((kind='password_reset' AND scope IS NULL AND session_id IS NULL)
      OR (kind='session_revoke' AND scope IS NOT NULL AND (scope='session')=(session_id IS NOT NULL)))
);
CREATE INDEX account_support_actor ON account_support_operations(actor_id,completed_at);

ALTER TABLE web_admin_events
    ADD COLUMN operation_id uuid,
    ADD COLUMN result text NOT NULL DEFAULT 'historical_unknown'
      CHECK (result IN ('historical_unknown','succeeded','denied','failed')),
    ADD COLUMN details jsonb CHECK (details IS NULL OR (jsonb_typeof(details)='object' AND octet_length(details::text)<=4096));
CREATE INDEX web_admin_operation ON web_admin_events(operation_id) WHERE operation_id IS NOT NULL;
