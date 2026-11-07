-- W12.1 残留：移除竞价事务内对追加式 `bids` 的范围聚合读取。
--
-- 背景：`enforce_bid_fact_sequence()` 每插入一条竞价事实都要执行
-- `SELECT max(fact_index) FROM public.bids WHERE command_id = NEW.command_id`。
-- 这是一次范围扫描，在 Serializable 下会在 `bids` 索引页上取得页级谓词锁；而每个竞价
-- 事务都会向同一索引插入自己的事实行，于是逻辑上互不相关的两个命令（不同 command_id、
-- 不同 lot）形成双向 rw 反依赖，PostgreSQL 以 40001 中止其中一方，重试预算耗尽后
-- Public 出价返回 503。冲突不是业务争用，而是 SSI 误报。
--
-- 不变量：
-- 1. 竞价事实仍只追加，`bids` 的 INSERT-only 授权与 append-only 触发器保持不变。
-- 2. 同一命令的事实序号仍然从 1 开始连续且由数据库强制；不连续或命令不存在仍以
--    23514/23503 拒绝，错误形状与旧触发器一致。
-- 3. 命令行仍以 `FOR UPDATE` 锁定后再比对，冲突判定与旧实现等价。
--
-- 做法：把“上一条事实序号”提升为 `auction_commands.last_bid_fact_index` 类型化列，
-- 触发器改为锁定该命令行后读取并推进这一列。写入只落在本事务自己的命令行上，
-- 不再扫描共享索引页。
--
-- 同一迁移补齐第二个 W12.1 残留：`lot_watch_leadership` 的懒初始化。
-- 旧实现先对不存在的领先周期行做 `SELECT … FOR UPDATE`（写侧页谓词锁），再 INSERT 同一
-- 主键；跨 lot 并发首笔出价因此同样互相中止。这里让行随 lot 创建事务一起产生，并为
-- 迁移之后创建、尚未初始化的历史 lot 回填（只由 `lots` 现有事实重建，cycle 取首次状态版本，
-- 与迁移 000041 的形状一致）。迁移之后所有 lot 都必然有领先周期行，竞价事务不再需要
-- 锁定读取未命中的行。
--
-- 兼容登记：替换既有触发器属于收紧/改写既有约束语义，旧程序（不写该列的版本）无法
-- 维持该计数，因此登记 breaking；迁移只在停机门禁下执行，旧 Worker 必须先停止。

LOCK TABLE public.auction_commands IN SHARE ROW EXCLUSIVE MODE;
LOCK TABLE public.bids IN SHARE ROW EXCLUSIVE MODE;

ALTER TABLE public.auction_commands
    ADD COLUMN last_bid_fact_index integer NOT NULL DEFAULT 0
        CHECK (last_bid_fact_index >= 0);

-- 回填：只由追加式事实重建，不猜测；没有事实的命令保持 0。
UPDATE public.auction_commands AS command
SET last_bid_fact_index = fact.max_fact_index
FROM (
    SELECT command_id, max(fact_index) AS max_fact_index
    FROM public.bids
    GROUP BY command_id
) AS fact
WHERE fact.command_id = command.command_id;

CREATE OR REPLACE FUNCTION public.enforce_bid_fact_sequence()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    last_fact_index integer;
BEGIN
    -- 命令行是本事务自己刚写入或已存在的账本行；锁定它只防止同 ID 并发事实交错。
    SELECT auction_commands.last_bid_fact_index
    INTO last_fact_index
    FROM public.auction_commands
    WHERE auction_commands.command_id = NEW.command_id
    FOR UPDATE;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'auction command does not exist for bid fact'
            USING ERRCODE = '23503';
    END IF;

    IF NEW.fact_index <> last_fact_index + 1 THEN
        RAISE EXCEPTION 'bid fact sequence must be contiguous'
            USING ERRCODE = '23514';
    END IF;

    UPDATE public.auction_commands
    SET last_bid_fact_index = NEW.fact_index
    WHERE command_id = NEW.command_id;

    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION public.create_lot_watch_leadership()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO public.lot_watch_leadership (lot_id, leader_user_id, cycle)
    VALUES (NEW.id, NEW.leader_user_id, NEW.state_version)
    ON CONFLICT (lot_id) DO NOTHING;
    RETURN NEW;
END;
$$;

CREATE TRIGGER lots_create_watch_leadership
AFTER INSERT ON public.lots
FOR EACH ROW EXECUTE FUNCTION public.create_lot_watch_leadership();

INSERT INTO public.lot_watch_leadership (lot_id, leader_user_id, cycle)
SELECT lot.id, lot.leader_user_id, lot.state_version
FROM public.lots AS lot
WHERE NOT EXISTS (
    SELECT 1 FROM public.lot_watch_leadership AS leadership
    WHERE leadership.lot_id = lot.id
);

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000090', 'system.migration', 'migration',
    '000090', 'success', 'system',
    jsonb_build_object(
        'summary', 'bid fact counter and eager lot watch leadership replace cross-lot serializable hotspots',
        'compatibility', 'breaking')
);

-- 替换既有触发器语义：旧程序不维护 last_bid_fact_index，登记 breaking。
INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (90, 'breaking');
