-- Fresh development schema. Discard databases from earlier development builds.
-- There is deliberately no legacy data migration.
CREATE TABLE server_settings (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    default_user_mbps integer NOT NULL CHECK (default_user_mbps > 0),
    default_guest_mbps integer NOT NULL CHECK (default_guest_mbps > 0),
    policy_revision bigint NOT NULL DEFAULT 1 CHECK (policy_revision > 0),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE users (
    id uuid PRIMARY KEY,
    username text NOT NULL,
    server_admin boolean NOT NULL DEFAULT false,
    relay_limit_mbps integer CHECK (relay_limit_mbps > 0),
    username_key text NOT NULL UNIQUE,
    password_hash text NOT NULL,
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    auth_revision bigint NOT NULL DEFAULT 1 CHECK (auth_revision > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE tenants (
    id uuid PRIMARY KEY,
    kind text NOT NULL CHECK (kind IN ('personal', 'unclaimed_device', 'guest')),
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'archived')),
    revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_by_user_id uuid REFERENCES users(id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE personal_tenants (
    tenant_id uuid PRIMARY KEY REFERENCES tenants(id) ON DELETE RESTRICT,
    user_id uuid NOT NULL UNIQUE REFERENCES users(id) ON DELETE RESTRICT
);

CREATE TABLE devices (
    id uuid PRIMARY KEY,
    code integer NOT NULL UNIQUE CHECK (code BETWEEN 100000000 AND 999999999),
    owner_tenant_id uuid REFERENCES tenants(id) ON DELETE RESTRICT,
    last_online_at timestamptz,
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    name text NOT NULL CHECK (length(name) BETWEEN 1 AND 128),
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
    registered_by_user_id uuid REFERENCES users(id) ON DELETE RESTRICT,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, id)
);

CREATE TABLE endpoints (
    endpoint_key bytea PRIMARY KEY CHECK (octet_length(endpoint_key) = 32),
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    owner_kind text NOT NULL CHECK (owner_kind IN ('user', 'device', 'guest')),
    user_id uuid,
    device_id uuid,
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'revoked')),
    revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (
        (owner_kind = 'user' AND user_id IS NOT NULL AND device_id IS NULL)
        OR
        (owner_kind = 'device' AND user_id IS NULL AND device_id IS NOT NULL)
        OR (owner_kind = 'guest' AND user_id IS NULL AND device_id IS NULL)
    ),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE RESTRICT,
    FOREIGN KEY (tenant_id, device_id)
        REFERENCES devices(tenant_id, id) ON DELETE RESTRICT
);

CREATE INDEX endpoints_active_tenant
    ON endpoints (tenant_id)
    WHERE status = 'active';


CREATE TABLE device_runtime (
    tenant_id uuid NOT NULL,
    device_id uuid NOT NULL,
    execution_context jsonb NOT NULL,
    environment_revision text NOT NULL CHECK (length(environment_revision) BETWEEN 1 AND 128),
    agent_version text NOT NULL CHECK (length(agent_version) BETWEEN 1 AND 64),
    observed_at_unix_ms bigint NOT NULL,
    accepted_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, device_id),
    FOREIGN KEY (tenant_id, device_id)
        REFERENCES devices(tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE device_network (
    tenant_id uuid NOT NULL,
    device_id uuid NOT NULL,
    endpoint_key bytea NOT NULL CHECK (octet_length(endpoint_key) = 32),
    endpoint_instance_id uuid NOT NULL,
    address_revision bigint NOT NULL CHECK (address_revision > 0),
    relay_urls jsonb NOT NULL,
    direct_addresses jsonb NOT NULL,
    observed_at_unix_ms bigint NOT NULL CHECK (observed_at_unix_ms > 0),
    accepted_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, device_id),
    FOREIGN KEY (tenant_id, device_id)
        REFERENCES devices(tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE device_connection_intents (
    operator_endpoint_key bytea NOT NULL REFERENCES endpoints(endpoint_key) ON DELETE CASCADE,
    device_id uuid NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (operator_endpoint_key, device_id)
);
CREATE INDEX device_connection_intents_expiry ON device_connection_intents(expires_at);

CREATE TABLE web_sessions (
    token_hash text PRIMARY KEY CHECK (length(token_hash) = 64),
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX web_sessions_user ON web_sessions(user_id);

CREATE TABLE endpoint_user_contexts (
    endpoint_key bytea PRIMARY KEY REFERENCES endpoints(endpoint_key) ON DELETE CASCADE,
    token_hash text REFERENCES web_sessions(token_hash) ON DELETE SET NULL,
    revision bigint NOT NULL CHECK (revision >= 0),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX endpoint_user_context_session ON endpoint_user_contexts(token_hash);

CREATE TABLE web_admin_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_id uuid REFERENCES users(id) ON DELETE SET NULL,
    action text NOT NULL,
    resource_id uuid,
    created_at timestamptz NOT NULL DEFAULT now()
);
-- Deliberately no arbitrary JSON/body: never persist passwords or task data.


CREATE TABLE relay_nodes (
    node_id text PRIMARY KEY CHECK(length(node_id) BETWEEN 1 AND 64),
    agent_version text,
    applied_policy_version bigint CHECK(applied_policy_version >= 0),
    offered_policy_version bigint NOT NULL CHECK(offered_policy_version >= 0),
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    server_instance uuid NOT NULL
);
