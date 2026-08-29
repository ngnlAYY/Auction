-- Auction-BF v28 的固定 INSERT 不包含后来加入的 bids.effective_end_at。旧事务会先
-- 锁定并更新 lot 的同一事实值，因此只为与该投影逐字段一致的遗漏列恢复权威值。
CREATE FUNCTION public.fill_legacy_bid_effective_end_at()
RETURNS trigger
LANGUAGE plpgsql
SECURITY INVOKER
SET search_path = pg_catalog
AS $$
DECLARE
    resolved_effective_end_at timestamptz;
BEGIN
    SELECT lot.effective_end_at
    INTO resolved_effective_end_at
    FROM public.lots AS lot
    WHERE lot.id = NEW.lot_id
      AND lot.current_price = NEW.amount
      AND lot.leader_user_id = NEW.bidder_user_id
      AND lot.state_version = NEW.accepted_state_version;

    IF resolved_effective_end_at IS NULL THEN
        RAISE EXCEPTION
            'legacy bid insert does not match the projected lot state'
            USING ERRCODE = '23514';
    END IF;

    NEW.effective_end_at := resolved_effective_end_at;
    RETURN NEW;
END
$$;

CREATE TRIGGER bids_fill_legacy_effective_end_at
BEFORE INSERT ON public.bids
FOR EACH ROW
WHEN (NEW.effective_end_at IS NULL)
EXECUTE FUNCTION public.fill_legacy_bid_effective_end_at();
