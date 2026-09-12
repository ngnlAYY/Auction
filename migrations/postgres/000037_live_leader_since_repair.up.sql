-- 修正普通出价被同一命令内的快速代理反超时，错误保留事务前领先起点的投影。
-- 仅重建尚未结算的 live lot；接受事实、已结算金额与历史审计均保持原样。
-- 迁移与业务写入互斥，保证重建依据和目标状态来自同一稳定事实集合。
LOCK TABLE public.auctions, public.lots, public.bids IN SHARE ROW EXCLUSIVE MODE;

WITH reconstructed AS (
    SELECT lot.id AS lot_id,
           (
               SELECT MIN(bid.created_at)
               FROM public.bids AS bid
               WHERE bid.lot_id = lot.id
                 AND bid.bidder_user_id = lot.leader_user_id
                 AND bid.accepted_state_version > COALESCE(
                     (
                         SELECT MAX(previous.accepted_state_version)
                         FROM public.bids AS previous
                         WHERE previous.lot_id = lot.id
                           AND previous.bidder_user_id <> lot.leader_user_id
                     ),
                     0
                 )
           ) AS leader_since
    FROM public.lots AS lot
    INNER JOIN public.auctions AS auction ON auction.id = lot.auction_id
    WHERE lot.status = 'live'
      AND auction.status IN ('live', 'final', 'settling')
      AND lot.leader_user_id IS NOT NULL
      AND NOT EXISTS (
          SELECT 1 FROM public.lot_settlements AS settlement WHERE settlement.lot_id = lot.id
      )
)
UPDATE public.lots AS lot
SET leader_since = reconstructed.leader_since
FROM reconstructed
WHERE lot.id = reconstructed.lot_id
  AND reconstructed.leader_since IS NOT NULL
  AND reconstructed.leader_since <= lot.effective_end_at
  AND lot.leader_since IS DISTINCT FROM reconstructed.leader_since;
