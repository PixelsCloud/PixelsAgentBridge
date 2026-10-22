-- Additive only: historical events are never backfilled with current names or values.
ALTER TABLE web_admin_events
    ADD COLUMN format_version smallint NOT NULL DEFAULT 0 CHECK (format_version IN (0,1)),
    ADD COLUMN request_id uuid,
    ADD COLUMN actor_ref uuid,
    ADD COLUMN actor_name text CHECK (length(actor_name)<=128),
    ADD COLUMN resource_kind text CHECK (resource_kind IN ('account','device','relay','server','audit','unknown')),
    ADD COLUMN resource_key text CHECK (length(resource_key)<=128),
    ADD COLUMN resource_name text CHECK (length(resource_name)<=128),
    ADD COLUMN before_values jsonb CHECK (before_values IS NULL OR (jsonb_typeof(before_values)='object' AND octet_length(before_values::text)<=16384)),
    ADD COLUMN after_values jsonb CHECK (after_values IS NULL OR (jsonb_typeof(after_values)='object' AND octet_length(after_values::text)<=16384)),
    ADD COLUMN insert_xid xid8;
-- Separate SET DEFAULT avoids rewriting/backfilling old events. The migration's
-- table lock makes all NULL-xid rows committed history before new writes resume.
ALTER TABLE web_admin_events ALTER COLUMN insert_xid SET DEFAULT pg_current_xact_id();
CREATE INDEX web_admin_action_sequence ON web_admin_events(action,id DESC);
CREATE INDEX web_admin_actor_sequence ON web_admin_events(actor_ref,id DESC);
CREATE INDEX web_admin_resource_sequence ON web_admin_events(resource_kind,resource_key,id DESC);
CREATE INDEX web_admin_result_sequence ON web_admin_events(result,id DESC);
CREATE INDEX web_admin_time_sequence ON web_admin_events(created_at,id DESC);

-- One retained rate-limit slot per actor/action/error class, not per requested
-- target or request ID. No event history is deleted by this feature.
CREATE TABLE audit_failure_limits (
    actor_key uuid NOT NULL,
    action text NOT NULL CHECK (length(action)<=64),
    error_class text NOT NULL CHECK (error_class IN ('unauthorized','forbidden','invalid_input','conflict','not_found','rate_limited','server_error')),
    recorded_at timestamptz NOT NULL,
    PRIMARY KEY(actor_key,action,error_class)
);
