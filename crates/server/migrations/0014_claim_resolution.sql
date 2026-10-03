ALTER TABLE device_claim_requests ADD COLUMN resolution text NOT NULL DEFAULT 'pending'
    CHECK (resolution IN ('pending','approved','rejected','cancelled'));
UPDATE device_claim_requests SET resolution='approved' WHERE approved_at IS NOT NULL;
CREATE INDEX device_claim_requests_requester ON device_claim_requests(requested_by_user_id,created_at DESC);
