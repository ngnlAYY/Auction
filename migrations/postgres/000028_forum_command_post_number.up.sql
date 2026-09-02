ALTER TABLE public.auction_forum_bid_commands
    ADD COLUMN post_number integer
        CHECK (post_number >= 3);

COMMENT ON COLUMN public.auction_forum_bid_commands.post_number IS
    '完整主题读取时观察到的论坛楼层号；历史无法证明的记录保留 NULL，禁止用 post_id 猜测。';
