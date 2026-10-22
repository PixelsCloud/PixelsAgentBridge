-- Additive: existing single-node and cluster policy handshakes remain valid.
ALTER TABLE relay_nodes
    ADD COLUMN public_url text,
    ADD COLUMN telemetry_boot_id uuid,
    ADD COLUMN telemetry_server_instance uuid,
    ADD COLUMN telemetry_session_started_at timestamptz,
    ADD COLUMN telemetry_received_at timestamptz,
    ADD COLUMN telemetry_connections numeric(20,0)
        CHECK(telemetry_connections BETWEEN 0 AND 18446744073709551615),
    ADD COLUMN telemetry_received_bytes numeric(20,0)
        CHECK(telemetry_received_bytes BETWEEN 0 AND 18446744073709551615),
    ADD COLUMN telemetry_sent_bytes numeric(20,0)
        CHECK(telemetry_sent_bytes BETWEEN 0 AND 18446744073709551615);
