-- Existing Debug device records must be cleared before applying this schema.
-- The public code is a locator, not an authentication credential.
ALTER TABLE devices
    ADD COLUMN code integer NOT NULL UNIQUE
    CHECK (code BETWEEN 100000000 AND 999999999);
