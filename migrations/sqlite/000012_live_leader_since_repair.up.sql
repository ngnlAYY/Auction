-- 与完整档同义：仅修正未结算 live lot 的连续领先起点，不修改追加式事实或历史终态。
-- SQLite 迁移事务串行写入；无法由接受事实证明的记录保持原样，不猜测时间。
WITH reconstructed AS (
    SELECT lot.id AS lot_id,
           (
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
           ) AS leader_since
    FROM lots AS lot
    INNER JOIN auctions AS auction ON auction.id = lot.auction_id
    WHERE lot.status = 'live'
      AND auction.status IN ('live', 'final', 'settling')
      AND lot.leader_user_id IS NOT NULL
)
UPDATE lots AS lot
SET leader_since = reconstructed.leader_since
FROM reconstructed
WHERE lot.id = reconstructed.lot_id
  AND reconstructed.leader_since IS NOT NULL
  AND julianday(reconstructed.leader_since) <= julianday(lot.effective_end_at)
  AND lot.leader_since IS NOT reconstructed.leader_since;
