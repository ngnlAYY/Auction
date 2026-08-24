-- 修复早期 Lite 竞价投影未维护 leader_since 的历史数据。
-- 只从追加式 bid 事实重建当前连续领先区间；无法证明的旧行保持 NULL。
UPDATE lots AS lot
SET leader_since = (
    SELECT MIN(bid.created_at)
    FROM bids AS bid
    WHERE bid.lot_id = lot.id
      AND bid.bidder_user_id = lot.leader_user_id
      AND bid.accepted_state_version > COALESCE(
          (
              SELECT MAX(previous.accepted_state_version)
              FROM bids AS previous
              WHERE previous.lot_id = lot.id
                AND previous.bidder_user_id <> lot.leader_user_id
          ),
          0
      )
)
WHERE lot.leader_user_id IS NOT NULL
  AND lot.leader_since IS NULL
  AND EXISTS (
      SELECT 1
      FROM bids AS bid
      WHERE bid.lot_id = lot.id
        AND bid.bidder_user_id = lot.leader_user_id
        AND bid.accepted_state_version > COALESCE(
            (
                SELECT MAX(previous.accepted_state_version)
                FROM bids AS previous
                WHERE previous.lot_id = lot.id
                  AND previous.bidder_user_id <> lot.leader_user_id
            ),
            0
        )
  );
