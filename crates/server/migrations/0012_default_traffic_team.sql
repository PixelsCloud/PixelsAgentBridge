ALTER TABLE users
    ADD COLUMN default_traffic_team_id uuid REFERENCES teams(tenant_id) ON DELETE RESTRICT;

CREATE TABLE account_traffic_assignment_events (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    old_team_id uuid REFERENCES teams(tenant_id) ON DELETE RESTRICT,
    new_team_id uuid REFERENCES teams(tenant_id) ON DELETE RESTRICT,
    operator_label text NOT NULL CHECK (length(operator_label) BETWEEN 1 AND 128),
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (old_team_id IS DISTINCT FROM new_team_id)
);

CREATE INDEX account_traffic_assignment_events_user_time
    ON account_traffic_assignment_events (user_id, created_at DESC);
