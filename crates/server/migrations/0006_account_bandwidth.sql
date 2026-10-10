-- Account bandwidth has a concrete default; no guest or configurable service quota.
ALTER TABLE server_settings DROP COLUMN default_user_mbps, DROP COLUMN default_guest_mbps;
UPDATE users SET relay_limit_mbps = 10
WHERE relay_limit_mbps IS NULL OR relay_limit_mbps NOT IN (5,10,20,30,40,50,60,70,80,90,100);
ALTER TABLE users ALTER COLUMN relay_limit_mbps SET DEFAULT 10;
ALTER TABLE users ALTER COLUMN relay_limit_mbps SET NOT NULL;
ALTER TABLE users ADD CONSTRAINT relay_limit_preset CHECK (relay_limit_mbps IN (5,10,20,30,40,50,60,70,80,90,100));
UPDATE server_settings SET policy_revision = policy_revision + 1 WHERE singleton;
