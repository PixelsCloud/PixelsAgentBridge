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
