-- Preview 不再冻结装备录入：每个世界的唯一 intake target 可以指向 Draft 或 Preview，
-- 但两种可接收状态在任意时刻合计最多只能有一个。迁移期间先阻止生命周期并发写入。
LOCK TABLE public.auctions, public.auction_intake_targets IN SHARE ROW EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT auction.world_id
        FROM public.auctions AS auction
        WHERE auction.status IN ('draft', 'preview')
        GROUP BY auction.world_id
        HAVING count(*) > 1
    ) THEN
        RAISE EXCEPTION
            'preview intake migration must run outside a legacy preview-to-open handoff window'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM public.auction_intake_targets AS target
        LEFT JOIN public.auctions AS auction
          ON auction.id = target.auction_id
         AND auction.world_id = target.world_id
         AND auction.status = target.target_status
        WHERE target.target_status NOT IN ('draft', 'preview')
           OR auction.id IS NULL
    ) THEN
        RAISE EXCEPTION
            'preview intake migration found a target without its exact intake auction'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM public.auctions AS auction
        LEFT JOIN public.auction_intake_targets AS target
          ON target.world_id = auction.world_id
         AND target.auction_id = auction.id
         AND target.target_status = auction.status
        WHERE auction.status IN ('draft', 'preview')
          AND target.world_id IS NULL
    ) THEN
        RAISE EXCEPTION
            'preview intake migration found an intake auction without its exact target'
            USING ERRCODE = '23514';
    END IF;
END
$$;

ALTER TABLE public.auction_intake_targets
    DROP CONSTRAINT auction_intake_targets_target_status_check,
    ADD CONSTRAINT auction_intake_targets_target_status_check
        CHECK (target_status IN ('draft', 'preview'));

CREATE UNIQUE INDEX auctions_one_intake_world_idx
    ON public.auctions (world_id)
    WHERE status IN ('draft', 'preview');

COMMENT ON COLUMN public.auction_intake_targets.target_status IS
    '当前可接收装备的拍卖状态；Preview 到正式开拍前仍属于本期录入目标';

DROP TRIGGER IF EXISTS auctions_draft_target_consistent ON public.auctions;
DROP TRIGGER IF EXISTS auction_intake_targets_draft_consistent
    ON public.auction_intake_targets;
DROP FUNCTION IF EXISTS public.validate_auction_draft_target_consistency();

-- 延迟检查允许同一事务先推进拍卖状态，再同步 target_status 或切换到下一期。唯一索引负责
-- 跨状态并发互斥；事务级 advisory lock 让来自非应用入口的跨表并发提交也不能漏过存在性检查。
CREATE FUNCTION public.validate_auction_intake_target_consistency()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    candidate_worlds text[];
    checked_world text;
    intake_count integer;
    target_count integer;
    intake_id uuid;
    target_id uuid;
    intake_status text;
    stored_target_status text;
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
        ORDER BY value
    LOOP
        PERFORM pg_advisory_xact_lock(
            hashtextextended('auction-intake-target:' || checked_world, 0)
        );

        SELECT count(*)::integer,
               (array_agg(id ORDER BY id))[1],
               (array_agg(status ORDER BY id))[1]
        INTO intake_count, intake_id, intake_status
        FROM public.auctions
        WHERE world_id = checked_world
          AND status IN ('draft', 'preview');

        SELECT count(*)::integer,
               (array_agg(auction_id ORDER BY auction_id))[1],
               (array_agg(target_status ORDER BY auction_id))[1]
        INTO target_count, target_id, stored_target_status
        FROM public.auction_intake_targets
        WHERE world_id = checked_world;

        IF NOT (
            (intake_count = 0 AND target_count = 0)
            OR (
                intake_count = 1
                AND target_count = 1
                AND intake_id = target_id
                AND intake_status = stored_target_status
            )
        ) THEN
            RAISE EXCEPTION
                'auction intake target is inconsistent for world %', checked_world
                USING ERRCODE = '23514';
        END IF;
    END LOOP;
    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER auctions_intake_target_consistent
AFTER INSERT OR DELETE OR UPDATE OF status ON public.auctions
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.validate_auction_intake_target_consistency();

CREATE CONSTRAINT TRIGGER auction_intake_targets_consistent
AFTER INSERT OR DELETE OR UPDATE OF world_id, auction_id, target_status
    ON public.auction_intake_targets
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.validate_auction_intake_target_consistency();
