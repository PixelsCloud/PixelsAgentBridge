-- Deployment defaults remain in TOML. NULL means inherit that default.
ALTER TABLE server_settings
    ADD COLUMN registration_override boolean,
    ADD COLUMN registration_revision bigint NOT NULL DEFAULT 1 CHECK (registration_revision > 0);
