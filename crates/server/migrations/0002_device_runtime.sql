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
