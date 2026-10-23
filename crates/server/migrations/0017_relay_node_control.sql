-- Confirmation belongs to one actual Relay process and one administrative action.
ALTER TABLE relay_cluster_nodes
    ADD COLUMN node_control_version integer NOT NULL DEFAULT 0,
    ADD COLUMN state_applied_boot_id uuid,
    ADD COLUMN state_applied_operation_id uuid,
    ADD COLUMN state_applied_admin_state text,
    ADD COLUMN state_applied_at timestamptz,
    ADD COLUMN state_drained boolean NOT NULL DEFAULT false;

-- Only maintenance needs a cluster-wide slot. Independent confirmations for
-- resume/disable must not prevent another node from being maintained.
DROP INDEX relay_one_maintenance_operation;
CREATE UNIQUE INDEX relay_one_maintenance_operation ON relay_maintenance_operations((true))
    WHERE action='maintenance' AND phase IN ('waiting','ready');
