-- auction owner：赛季早鸟窗口内场次冻结最早收件时刻（赛季开始）。赛季前最后一场开拍时即建立首场草稿，
-- 邮件收件、Admin 录入与恢复重挂都在锁内拒收早于该时刻的装备；该时刻同时是判定旧赛季装备
-- 不能再重挂的赛季边界。只在创建拍卖时写入，既有拍卖保持 NULL（不受限制），不回填。
ALTER TABLE public.auctions
    ADD COLUMN intake_opens_at timestamptz;

CREATE INDEX auctions_world_intake_opens_at_idx
    ON public.auctions (world_id, intake_opens_at)
    WHERE intake_opens_at IS NOT NULL;
