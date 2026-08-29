-- SQLite 不能向既有表追加 CHECK；用插入/更新触发器恢复同一 Lite 写入不变量。
CREATE TRIGGER lots_effective_end_shape_insert
BEFORE INSERT ON lots
WHEN NEW.status IN ('live', 'sold') AND NEW.effective_end_at IS NULL
BEGIN
    SELECT RAISE(ABORT, 'live or sold lot requires effective_end_at');
END;

CREATE TRIGGER lots_effective_end_shape_update
BEFORE UPDATE OF status, effective_end_at ON lots
WHEN NEW.status IN ('live', 'sold') AND NEW.effective_end_at IS NULL
BEGIN
    SELECT RAISE(ABORT, 'live or sold lot requires effective_end_at');
END;

-- 对非法旧行执行一次会命中新触发器的 no-op；存在残留时整份迁移失败关闭，不猜测截止时间。
UPDATE lots
SET status = status
WHERE status IN ('live', 'sold')
  AND effective_end_at IS NULL;
