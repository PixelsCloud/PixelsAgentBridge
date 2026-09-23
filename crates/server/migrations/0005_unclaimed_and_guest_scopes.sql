-- A technical scope keeps device/task references stable while ownership changes.
-- It is not an account or a Team, and does not grant membership permissions.
ALTER TABLE tenants DROP CONSTRAINT tenants_kind_check;
ALTER TABLE tenants ALTER COLUMN created_by_user_id DROP NOT NULL;
ALTER TABLE tenants ADD CONSTRAINT tenants_kind_creator_check CHECK (
    (kind IN ('personal', 'team') AND created_by_user_id IS NOT NULL)
    OR (kind IN ('unclaimed_device', 'guest') AND created_by_user_id IS NULL)
);

ALTER TABLE devices ALTER COLUMN registered_by_user_id DROP NOT NULL;
ALTER TABLE devices ADD COLUMN owner_tenant_id uuid REFERENCES tenants(id) ON DELETE RESTRICT;
UPDATE devices SET owner_tenant_id = tenant_id;
CREATE INDEX devices_owner_tenant ON devices(owner_tenant_id) WHERE owner_tenant_id IS NOT NULL;

ALTER TABLE endpoints DROP CONSTRAINT endpoints_owner_kind_check;
ALTER TABLE endpoints DROP CONSTRAINT endpoints_check;
ALTER TABLE endpoints ADD CONSTRAINT endpoints_owner_kind_check
    CHECK (owner_kind IN ('user', 'device', 'guest'));
ALTER TABLE endpoints ADD CONSTRAINT endpoints_owner_fields_check CHECK (
    (owner_kind = 'user' AND user_id IS NOT NULL AND device_id IS NULL)
    OR (owner_kind = 'device' AND user_id IS NULL AND device_id IS NOT NULL)
    OR (owner_kind = 'guest' AND user_id IS NULL AND device_id IS NULL)
);
