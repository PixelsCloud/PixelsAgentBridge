CREATE TABLE guest_device_intents (
    guest_endpoint_key bytea NOT NULL REFERENCES endpoints(endpoint_key) ON DELETE CASCADE,
    device_id uuid NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (guest_endpoint_key, device_id)
);
CREATE INDEX guest_device_intents_expiry ON guest_device_intents(expires_at);
