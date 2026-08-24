-- 修复早期 Rust 竞价投影未维护 leader_since 的历史数据。
-- 只从追加式 bid 事实重建当前连续领先区间；无法证明的旧行保持 NULL，并在结算时失败关闭。
WITH reconstructed AS (
    SELECT
        lot.id AS lot_id,
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
    WHERE lot.leader_user_id IS NOT NULL
      AND lot.leader_since IS NULL
)
UPDATE public.lots AS lot
SET leader_since = reconstructed.leader_since
FROM reconstructed
WHERE lot.id = reconstructed.lot_id
  AND reconstructed.leader_since IS NOT NULL;
