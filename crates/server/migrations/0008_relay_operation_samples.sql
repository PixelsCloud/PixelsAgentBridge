-- Existing operations have no recorded heartbeat window; their capacity is unknown.
-- New operations retain the decision window so subsequent reads use the same policy.
ALTER TABLE relay_maintenance_operations
    ADD COLUMN offline_seconds bigint CHECK (offline_seconds > 0),
    ADD COLUMN server_instance uuid;
