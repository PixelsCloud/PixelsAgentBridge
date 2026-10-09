CREATE TABLE user_catalog_versions (
    user_id uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    revision bigint NOT NULL DEFAULT 0 CHECK(revision>=0)
);
CREATE TABLE user_saved_devices (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id uuid NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    alias text NOT NULL DEFAULT '' CHECK(length(alias)<=128),
    revision bigint NOT NULL CHECK(revision>0),
    deleted boolean NOT NULL DEFAULT false,
    saved_name text NOT NULL,
    saved_system text,
    verified_until timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(user_id,device_id)
);
CREATE INDEX user_saved_devices_changes ON user_saved_devices(user_id,revision);
CREATE TABLE user_catalog_mutations (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    id uuid NOT NULL,
    request jsonb NOT NULL,
    result jsonb NOT NULL,
    PRIMARY KEY(user_id,id)
);
