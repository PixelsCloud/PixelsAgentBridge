CREATE TABLE usage_receipts (
    source text NOT NULL,
    batch_id uuid NOT NULL,
    digest text NOT NULL,
    hour_unix_ms bigint NOT NULL,
    PRIMARY KEY(source,batch_id)
);
CREATE INDEX usage_receipts_expiry ON usage_receipts(hour_unix_ms);
CREATE TABLE usage_hourly (
    source_kind text NOT NULL CHECK(source_kind IN ('client','relay')),
    subject text NOT NULL,
    hour_unix_ms bigint NOT NULL,
    counters jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(source_kind,subject,hour_unix_ms)
);
CREATE INDEX usage_hourly_range ON usage_hourly(hour_unix_ms,subject);
CREATE TABLE usage_daily (LIKE usage_hourly INCLUDING ALL);
CREATE TABLE usage_lifetime (LIKE usage_hourly INCLUDING ALL);
