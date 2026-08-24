-- Lite 轮次同样持久化 Final phase 完成 fence；动态 due 仍由 lot 有效结束时间和冻结策略计算。
ALTER TABLE scheduled_rounds ADD COLUMN final_command_id BLOB;
ALTER TABLE scheduled_rounds ADD COLUMN final_completed_at TEXT;

CREATE UNIQUE INDEX scheduled_rounds_final_command_idx
    ON scheduled_rounds (final_command_id)
    WHERE final_command_id IS NOT NULL;

CREATE INDEX scheduled_rounds_final_worker_due_idx
    ON scheduled_rounds (lease_until, open_completed_at, id)
    WHERE status = 'active' AND final_completed_at IS NULL;

CREATE TRIGGER scheduled_rounds_final_shape_insert
BEFORE INSERT ON scheduled_rounds
WHEN (NEW.final_completed_at IS NULL) <> (NEW.final_command_id IS NULL)
    OR (NEW.final_completed_at IS NOT NULL AND NEW.open_completed_at IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'scheduled round final phase shape is invalid');
END;

CREATE TRIGGER scheduled_rounds_final_shape_update
BEFORE UPDATE OF final_command_id, final_completed_at, open_completed_at ON scheduled_rounds
WHEN (NEW.final_completed_at IS NULL) <> (NEW.final_command_id IS NULL)
    OR (NEW.final_completed_at IS NOT NULL AND NEW.open_completed_at IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'scheduled round final phase shape is invalid');
END;
