-- Administrative capacity edits survive TOML reloads and service restarts.
-- NULL continues to inherit the deployment's TOML value.
ALTER TABLE relay_cluster_nodes ADD COLUMN capacity_override_mbps integer
    CHECK (capacity_override_mbps IS NULL OR capacity_override_mbps > 0);
