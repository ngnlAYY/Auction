-- W12.1 同场出价序号热点根治：竞价的公开事件改为“与出价同事务写入、提交后分配序号”。
-- owner: auction-storage（`realtime_events`、`realtime_streams` 的唯一 SQL owner）、
--        auction-application（relay 每轮兜底补齐待定序号）
-- 背景：旧实现在 Serializable 出价事务内递增同一 `auction:<id>` 的 `realtime_streams` 行。
--   同场不同 lot 的两次重叠出价都会读写该行，形成 SSI 读写反依赖，必有一方以 40001 中止；
--   Final 阶段出价密集时 3 次重试可能耗尽（D5）。
-- 不变量：
--   1. 被接受的出价事实、版本、追加审计与持久事件仍必须同事务写入（不变量 3）。本迁移只
--      允许 `stream_sequence` 暂时为 NULL；事件行本身与其内容仍与出价同一事务提交。
--   2. `realtime_events` 内容仍只追加：`stream_sequence` 只能从 NULL 分配成序号一次，
--      其余列由 BEFORE UPDATE 触发器拒绝改写。
--   3. 序号连续性仍是契约：分配器只从 `realtime_streams.next_sequence` 单向取下一个值，
--      因此不会产生缺口；未分配序号的事件不进入回放与 relay（`stream_sequence > after`
--      对 NULL 不成立）。
--   4. 本迁移不改写任何既有行、序号或游标；已分配事件全部保持原序号与原始顺序。
-- 等级：breaking。旧程序不会分配待定行，回退到旧程序后这些事件将永久留在回放之外；
--   因此不得在存在待定行（`stream_sequence IS NULL`）时回退到 085 及更早的程序。
-- 停机顺序：与其它迁移相同，在停机门禁下先停止三个角色再迁移；本迁移只放宽一列并新增索引与
--   触发器，不停机也不会改写既有事实，但仍按部署规范执行。
LOCK TABLE public.realtime_events IN SHARE ROW EXCLUSIVE MODE;

ALTER TABLE public.realtime_events ALTER COLUMN stream_sequence DROP NOT NULL;

-- 旧 CHECK（`stream_sequence > 0`）对 NULL 求值为 unknown 因而放行；这里显式命名新的闭集
-- 约束，使“已分配序号必须为正、未分配为 NULL”成为唯一被数据库接受的两态。
ALTER TABLE public.realtime_events
    DROP CONSTRAINT IF EXISTS realtime_events_stream_sequence_check,
    ADD CONSTRAINT realtime_events_stream_sequence_check
        CHECK (stream_sequence IS NULL OR stream_sequence > 0);

-- 分配器只扫描待定行，并严格按 (created_at, event_id) 取头部；部分索引避免全表排序，
-- 也让“是否有待定行”的目录扫描保持索引规模。
CREATE INDEX realtime_events_pending_sequence_idx
    ON public.realtime_events (stream_key, created_at, event_id)
    WHERE stream_sequence IS NULL;

-- 唯一允许的 UPDATE 是把 stream_sequence 从 NULL 分配成序号；事件内容不可被改写，
-- 已分配序号不可再分配（不变量 2）。
CREATE FUNCTION public.realtime_events_assign_sequence_only() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.stream_sequence IS NOT NULL THEN
        RAISE EXCEPTION 'realtime_events.stream_sequence 只能分配一次';
    END IF;
    IF NEW.stream_sequence IS NULL THEN
        RAISE EXCEPTION 'realtime_events.stream_sequence 分配结果不能为空';
    END IF;
    IF NEW.event_id <> OLD.event_id
        OR NEW.stream_key <> OLD.stream_key
        OR NEW.kind <> OLD.kind
        OR NEW.schema_version <> OLD.schema_version
        OR NEW.public_payload <> OLD.public_payload
        OR NEW.state_version IS DISTINCT FROM OLD.state_version
        OR NEW.origin_command_id IS DISTINCT FROM OLD.origin_command_id
        OR NEW.origin_bid_id IS DISTINCT FROM OLD.origin_bid_id
        OR NEW.origin_fact_index IS DISTINCT FROM OLD.origin_fact_index
        OR NEW.replay_of_event_id IS DISTINCT FROM OLD.replay_of_event_id
        OR NEW.created_at <> OLD.created_at
    THEN
        RAISE EXCEPTION 'realtime_events 只允许分配 stream_sequence，事件内容不可改写';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER realtime_events_deferred_sequence
BEFORE UPDATE ON public.realtime_events
FOR EACH ROW EXECUTE FUNCTION public.realtime_events_assign_sequence_only();

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (86, 'breaking');
