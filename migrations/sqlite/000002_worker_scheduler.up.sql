-- Worker 租约和失败状态只追加到既有轮次表；due 与租约仍由数据库条件更新决定。
ALTER TABLE scheduled_rounds ADD COLUMN lease_owner TEXT;
ALTER TABLE scheduled_rounds ADD COLUMN lease_until TEXT;
ALTER TABLE scheduled_rounds ADD COLUMN attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0);
ALTER TABLE scheduled_rounds ADD COLUMN last_failure_code TEXT;
ALTER TABLE scheduled_rounds ADD COLUMN last_failure_detail TEXT;

CREATE INDEX scheduled_rounds_worker_due_idx
    ON scheduled_rounds (lease_until, preview_due_at, open_due_at, id)
    WHERE status = 'active' AND open_completed_at IS NULL;
