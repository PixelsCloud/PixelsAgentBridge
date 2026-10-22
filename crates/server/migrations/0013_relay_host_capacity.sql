-- Zero is an unspecified node cap: use host bandwidth while retaining all
-- account budget leases. Existing positive capacities keep their meaning.
ALTER TABLE relay_cluster_nodes DROP CONSTRAINT relay_cluster_nodes_capacity_mbps_check;
ALTER TABLE relay_cluster_nodes ADD CONSTRAINT relay_cluster_nodes_capacity_mbps_check CHECK(capacity_mbps >= 0);
