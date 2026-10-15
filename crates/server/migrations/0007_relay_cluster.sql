-- Additive: legacy relay_nodes/schema-6 operation remains available during rollout.
CREATE TABLE relay_cluster_state (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
    enabled boolean NOT NULL DEFAULT false,
    revision bigint NOT NULL DEFAULT 1 CHECK(revision > 0)
);
INSERT INTO relay_cluster_state DEFAULT VALUES;

CREATE TABLE relay_cluster_nodes (
    node_id text PRIMARY KEY,
    public_url text NOT NULL UNIQUE,
    region text NOT NULL DEFAULT '',
    weight integer NOT NULL CHECK(weight BETWEEN 1 AND 1000),
    capacity_mbps integer NOT NULL CHECK(capacity_mbps > 0),
    configured boolean NOT NULL DEFAULT true,
    admin_state text NOT NULL DEFAULT 'active' CHECK(admin_state IN ('active','maintenance','disabled')),
    health text NOT NULL DEFAULT 'starting' CHECK(health IN ('starting','healthy','suspect','offline','incompatible')),
    boot_id uuid,
    retired boolean NOT NULL DEFAULT false,
    agent_version text,
    healthy_since timestamptz,
    last_seen_at timestamptz,
    maintenance_started_at timestamptz,
    operation_id uuid,
    server_instance uuid,
    connections bigint NOT NULL DEFAULT 0 CHECK(connections >= 0),
    uploaded_bytes bigint NOT NULL DEFAULT 0 CHECK(uploaded_bytes >= 0),
    downloaded_bytes bigint NOT NULL DEFAULT 0 CHECK(downloaded_bytes >= 0)
);

CREATE TABLE relay_budget_leases (
    user_id uuid NOT NULL REFERENCES users(id),
    node_id text NOT NULL REFERENCES relay_cluster_nodes(node_id),
    boot_id uuid NOT NULL,
    generation bigserial UNIQUE NOT NULL,
    bytes_per_second bigint NOT NULL CHECK(bytes_per_second >= 0),
    burst_bytes bigint NOT NULL CHECK(burst_bytes >= 0),
    expires_at timestamptz NOT NULL,
    last_demand_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(user_id,node_id,boot_id)
);
CREATE INDEX relay_budget_active ON relay_budget_leases(user_id,expires_at);
CREATE SEQUENCE relay_budget_snapshot_revision;

CREATE TABLE relay_maintenance_operations (
    id uuid PRIMARY KEY,
    node_id text NOT NULL REFERENCES relay_cluster_nodes(node_id),
    action text NOT NULL CHECK(action IN ('maintenance','activate','disable')),
    phase text NOT NULL CHECK(phase IN ('waiting','ready','completed','failed')),
    allow_outage boolean NOT NULL DEFAULT false,
    started_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    deadline_at timestamptz NOT NULL,
    completed_at timestamptz,
    actor_id uuid NOT NULL REFERENCES users(id),
    error_code text
);
CREATE UNIQUE INDEX relay_one_maintenance_operation ON relay_maintenance_operations((true))
    WHERE phase IN ('waiting','ready');
