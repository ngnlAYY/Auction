-- 生命周期聚合的跨行约束属于 PostgreSQL 完整档防线。迁移先阻止并发写入，拒绝无法
-- 唯一解释的投影，再修复早期 Rust 迁移遗漏的确定性 draft target 回填。
LOCK TABLE public.auctions, public.auction_intake_targets IN SHARE ROW EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM public.auctions AS successor
        JOIN public.auctions AS predecessor
          ON predecessor.id = successor.predecessor_auction_id
         AND predecessor.world_id = successor.world_id
        WHERE predecessor.sequence >= successor.sequence
    ) THEN
        RAISE EXCEPTION
            'lifecycle invariant migration requires predecessor sequence to be lower than successor sequence'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM public.auction_intake_targets AS target
        LEFT JOIN public.auctions AS auction
          ON auction.id = target.auction_id
         AND auction.world_id = target.world_id
         AND auction.status = target.target_status
        WHERE target.target_status <> 'draft' OR auction.id IS NULL
    ) THEN
        RAISE EXCEPTION
            'lifecycle invariant migration found an intake target without its exact draft auction'
            USING ERRCODE = '23514';
    END IF;
END
$$;

-- `000003` 在已有 Rust 数据上曾只创建 target 表而没有迁移旧 draft。每个世界已有唯一 draft
-- 索引，且上面的检查已经排除冲突 target，因此这部分映射是唯一且可恢复的。
INSERT INTO public.auction_intake_targets (
    world_id,
    auction_id,
    target_status,
    state_version,
    updated_at
)
SELECT
    auction.world_id,
    auction.id,
    'draft',
    1,
    transaction_timestamp()
FROM public.auctions AS auction
WHERE auction.status = 'draft'
  AND NOT EXISTS (
      SELECT 1
      FROM public.auction_intake_targets AS target
      WHERE target.world_id = auction.world_id
  );

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM public.auctions AS auction
        WHERE auction.status = 'draft'
          AND NOT EXISTS (
              SELECT 1
              FROM public.auction_intake_targets AS target
              WHERE target.world_id = auction.world_id
                AND target.auction_id = auction.id
                AND target.target_status = 'draft'
          )
    ) THEN
        RAISE EXCEPTION
            'lifecycle invariant migration could not establish the draft intake target projection'
            USING ERRCODE = '23514';
    END IF;
END
$$;

-- 拍卖标识、世界、序号和前序关系一经建立即是聚合身份；普通状态推进不得改写这些字段。
CREATE OR REPLACE FUNCTION public.reject_auction_identity_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.id IS DISTINCT FROM OLD.id
       OR NEW.world_id IS DISTINCT FROM OLD.world_id
       OR NEW.sequence IS DISTINCT FROM OLD.sequence
       OR NEW.predecessor_auction_id IS DISTINCT FROM OLD.predecessor_auction_id
    THEN
        RAISE EXCEPTION 'auction identity and predecessor are immutable'
            USING ERRCODE = '55000';
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS auctions_identity_immutable ON public.auctions;

CREATE TRIGGER auctions_identity_immutable
BEFORE UPDATE OF id, world_id, sequence, predecessor_auction_id ON public.auctions
FOR EACH ROW EXECUTE FUNCTION public.reject_auction_identity_mutation();

-- 复合外键只能证明前序属于同一世界；延迟触发器继续保证它确实早于后继拍卖。
CREATE OR REPLACE FUNCTION public.validate_auction_predecessor_sequence()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    predecessor_sequence bigint;
BEGIN
    IF NEW.predecessor_auction_id IS NULL THEN
        RETURN NEW;
    END IF;

    SELECT sequence
    INTO predecessor_sequence
    FROM public.auctions
    WHERE id = NEW.predecessor_auction_id
      AND world_id = NEW.world_id;

    IF predecessor_sequence IS NULL OR predecessor_sequence >= NEW.sequence THEN
        RAISE EXCEPTION 'auction predecessor sequence must be lower than successor sequence'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS auctions_predecessor_sequence_valid ON public.auctions;

CREATE CONSTRAINT TRIGGER auctions_predecessor_sequence_valid
AFTER INSERT OR UPDATE OF predecessor_auction_id, world_id, sequence ON public.auctions
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.validate_auction_predecessor_sequence();

-- 每个世界要么同时没有 draft 与 intake target，要么二者恰好指向同一聚合。延迟到事务
-- 提交前检查，允许 preview/cancel 在同一事务中先推进拍卖再移动或删除 target。
CREATE OR REPLACE FUNCTION public.validate_auction_draft_target_consistency()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    candidate_worlds text[];
    checked_world text;
    draft_count integer;
    target_count integer;
    draft_id uuid;
    target_id uuid;
BEGIN
    IF TG_OP = 'INSERT' THEN
        candidate_worlds := ARRAY[NEW.world_id];
    ELSIF TG_OP = 'DELETE' THEN
        candidate_worlds := ARRAY[OLD.world_id];
    ELSE
        candidate_worlds := ARRAY[OLD.world_id, NEW.world_id];
    END IF;

    FOR checked_world IN
        SELECT DISTINCT value
        FROM unnest(candidate_worlds) AS value
        WHERE value IS NOT NULL
    LOOP
        SELECT count(*)::integer, (array_agg(id ORDER BY id))[1]
        INTO draft_count, draft_id
        FROM public.auctions
        WHERE world_id = checked_world
          AND status = 'draft';

        SELECT count(*)::integer, (array_agg(auction_id ORDER BY auction_id))[1]
        INTO target_count, target_id
        FROM public.auction_intake_targets
        WHERE world_id = checked_world;

        IF NOT (
            (draft_count = 0 AND target_count = 0)
            OR (
                draft_count = 1
                AND target_count = 1
                AND draft_id = target_id
            )
        ) THEN
            RAISE EXCEPTION
                'auction draft and intake target are inconsistent for world %', checked_world
                USING ERRCODE = '23514';
        END IF;
    END LOOP;
    RETURN NULL;
END;
$$;

DROP TRIGGER IF EXISTS auctions_draft_target_consistent ON public.auctions;
DROP TRIGGER IF EXISTS auction_intake_targets_draft_consistent
    ON public.auction_intake_targets;

CREATE CONSTRAINT TRIGGER auctions_draft_target_consistent
AFTER INSERT OR DELETE OR UPDATE OF status ON public.auctions
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.validate_auction_draft_target_consistency();

CREATE CONSTRAINT TRIGGER auction_intake_targets_draft_consistent
AFTER INSERT OR DELETE OR UPDATE OF world_id, auction_id ON public.auction_intake_targets
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.validate_auction_draft_target_consistency();

-- 当前论坛同步由 Worker 在领取事务中发现已提交的生命周期版本。旧 v28 的 status 触发器会
-- 形成第二个隐含入队 owner；接管后的前向迁移必须移除它，fresh schema 上则保持无操作。
DROP TRIGGER IF EXISTS auctions_forum_sync_after_open ON public.auctions;
DROP FUNCTION IF EXISTS public.enqueue_auction_forum_sync();
