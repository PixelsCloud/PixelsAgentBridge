-- Internal agent crate versions are not installed product versions.
-- Leave historical records unknown until a client explicitly reports its version.
ALTER TABLE device_runtime ADD COLUMN client_version text;
