CREATE TABLE deployments (
    id uuid PRIMARY KEY,
    singleton boolean NOT NULL DEFAULT true UNIQUE CHECK (singleton),
    default_team_mbps integer NOT NULL CHECK (default_team_mbps > 0),
    default_member_mbps integer NOT NULL CHECK (default_member_mbps > 0),
    default_personal_mbps integer NOT NULL CHECK (default_personal_mbps > 0),
    policy_revision bigint NOT NULL DEFAULT 1 CHECK (policy_revision > 0),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE users (
    id uuid PRIMARY KEY,
    username text NOT NULL,
    username_key text NOT NULL UNIQUE,
    password_hash text NOT NULL,
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    auth_revision bigint NOT NULL DEFAULT 1 CHECK (auth_revision > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE tenants (
    id uuid PRIMARY KEY,
    kind text NOT NULL CHECK (kind IN ('personal', 'team')),
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'archived')),
    revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_by_user_id uuid NOT NULL REFERENCES users(id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE personal_tenants (
    tenant_id uuid PRIMARY KEY REFERENCES tenants(id) ON DELETE RESTRICT,
    user_id uuid NOT NULL UNIQUE REFERENCES users(id) ON DELETE RESTRICT
);

CREATE TABLE teams (
    tenant_id uuid PRIMARY KEY REFERENCES tenants(id) ON DELETE RESTRICT,
    name text NOT NULL CHECK (length(name) BETWEEN 1 AND 128),
    relay_total_mbps integer CHECK (relay_total_mbps > 0),
    relay_member_mbps integer CHECK (relay_member_mbps > 0)
);

CREATE TABLE memberships (
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    role text NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'removed')),
    revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, user_id)
);

CREATE UNIQUE INDEX memberships_one_active_owner
    ON memberships (tenant_id)
    WHERE role = 'owner' AND status = 'active';

CREATE INDEX memberships_user_active
    ON memberships (user_id, tenant_id)
    WHERE status = 'active';

CREATE TABLE team_invitations (
    id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL REFERENCES teams(tenant_id) ON DELETE RESTRICT,
    invited_user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    invited_by_user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    role text NOT NULL CHECK (role IN ('admin', 'member')),
    status text NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'accepted', 'declined', 'revoked')),
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    responded_at timestamptz,
    CHECK (expires_at > created_at)
);

CREATE UNIQUE INDEX team_invitations_one_pending
    ON team_invitations (tenant_id, invited_user_id)
    WHERE status = 'pending';

CREATE TABLE devices (
    id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    name text NOT NULL CHECK (length(name) BETWEEN 1 AND 128),
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
    registered_by_user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, id)
);

CREATE TABLE endpoints (
    endpoint_key bytea PRIMARY KEY CHECK (octet_length(endpoint_key) = 32),
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    owner_kind text NOT NULL CHECK (owner_kind IN ('user', 'device')),
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
    ),
    FOREIGN KEY (tenant_id, user_id)
        REFERENCES memberships(tenant_id, user_id) ON DELETE RESTRICT,
    FOREIGN KEY (tenant_id, device_id)
        REFERENCES devices(tenant_id, id) ON DELETE RESTRICT
);

CREATE INDEX endpoints_active_tenant
    ON endpoints (tenant_id)
    WHERE status = 'active';

CREATE TABLE device_grants (
    tenant_id uuid NOT NULL,
    device_id uuid NOT NULL,
    user_id uuid NOT NULL,
    capability_bits integer NOT NULL CHECK (capability_bits BETWEEN 0 AND 15),
    revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
    granted_by_user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, device_id, user_id),
    FOREIGN KEY (tenant_id, device_id)
        REFERENCES devices(tenant_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (tenant_id, user_id)
        REFERENCES memberships(tenant_id, user_id) ON DELETE RESTRICT
);

