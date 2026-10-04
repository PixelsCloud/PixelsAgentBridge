-- Claims are retired. Preserve existing identities, ownership and historical records.
-- Pending requests cannot be approved, including after rollback to an older server.
UPDATE device_claim_requests SET resolution = 'cancelled' WHERE resolution = 'pending';
