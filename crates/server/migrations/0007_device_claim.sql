ALTER TABLE device_grants DROP CONSTRAINT device_grants_tenant_id_device_id_fkey;
ALTER TABLE device_grants ADD CONSTRAINT device_grants_device_id_fkey
    FOREIGN KEY (device_id) REFERENCES devices(id) ON DELETE RESTRICT;

CREATE TABLE device_claim_requests (
    id uuid PRIMARY KEY,
    device_id uuid NOT NULL REFERENCES devices(id) ON DELETE RESTRICT,
    owner_tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    requested_by_user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    expires_at timestamptz NOT NULL,
    approved_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (expires_at > created_at)
);
CREATE INDEX device_claim_requests_pending ON device_claim_requests(device_id, expires_at)
    WHERE approved_at IS NULL;
