ALTER TABLE team_admin_events
    DROP CONSTRAINT team_admin_events_action_check;

ALTER TABLE team_admin_events
    ADD CONSTRAINT team_admin_events_action_check
    CHECK (action IN ('team_created', 'member_added', 'member_removed', 'limits_changed'));

ALTER TABLE team_admin_events
    ADD COLUMN old_total_mbps integer CHECK (old_total_mbps > 0),
    ADD COLUMN old_member_mbps integer CHECK (old_member_mbps > 0),
    ADD COLUMN new_total_mbps integer CHECK (new_total_mbps > 0),
    ADD COLUMN new_member_mbps integer CHECK (new_member_mbps > 0);
