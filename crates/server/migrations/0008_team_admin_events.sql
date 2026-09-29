CREATE TABLE team_admin_events (
    id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL REFERENCES teams(tenant_id) ON DELETE RESTRICT,
    target_user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    action text NOT NULL CHECK (action IN ('team_created', 'member_added')),
    role text NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
    operator_label text NOT NULL CHECK (length(operator_label) BETWEEN 1 AND 128),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX team_admin_events_team_time
    ON team_admin_events (tenant_id, created_at DESC);
