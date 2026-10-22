-- Operator-maintained host identity, distinct from container-local DNS results.
ALTER TABLE relay_cluster_nodes ADD COLUMN machine_ip inet;
