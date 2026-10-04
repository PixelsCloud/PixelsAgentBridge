CREATE TABLE relay_nodes (
    node_id text PRIMARY KEY CHECK(length(node_id) BETWEEN 1 AND 64),
    agent_version text,
    applied_policy_version bigint CHECK(applied_policy_version >= 0),
    offered_policy_version bigint NOT NULL CHECK(offered_policy_version >= 0),
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    server_instance uuid NOT NULL
);
