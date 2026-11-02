-- Change only accounts still using the old default with no successful manual
-- bandwidth override, including overrides explicitly set back to 10 Mbps.
ALTER TABLE users ALTER COLUMN relay_limit_mbps SET DEFAULT 5;
UPDATE users AS u SET relay_limit_mbps = 5, updated_at = now()
WHERE u.relay_limit_mbps = 10
  AND NOT EXISTS (
    SELECT 1 FROM web_admin_events AS e
    WHERE e.action = 'account.relay_limit_updated'
      AND e.result IN ('succeeded', 'historical_unknown')
      AND (e.resource_id = u.id
        OR (e.resource_kind = 'account' AND e.resource_key = u.id::text))
  );
UPDATE server_settings SET policy_revision = policy_revision + 1 WHERE singleton;
