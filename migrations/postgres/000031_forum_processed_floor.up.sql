-- 单独保存已完整处理的真实论坛楼层；不得以固定正文版本或远端 post ID 推算该水位。
ALTER TABLE public.auction_forum_sync
    ADD COLUMN processed_through_post_number integer NOT NULL DEFAULT 2
        CHECK (processed_through_post_number >= 2);

COMMENT ON COLUMN public.auction_forum_sync.processed_through_post_number IS
    '最近一次由完整主题读取证明已处理到的真实楼层号；固定 #1/#2 不计入用户命令起点';
