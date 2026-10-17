-- forum owner：终态纳入范围与真实完成证明分离，不回填升级前已结算帖子。
ALTER TABLE public.auction_forum_sync
    ADD COLUMN terminal_refresh_enabled boolean NOT NULL DEFAULT true,
    ADD COLUMN terminal_synced_version bigint,
    ADD CONSTRAINT auction_forum_terminal_version_shape CHECK (
        terminal_synced_version IS NULL
        OR (terminal_refresh_enabled AND terminal_synced_version > 0)
    );

-- 受控停机升级；历史豁免不是成功，完成版本必须保持 NULL。
UPDATE public.auction_forum_sync AS sync
SET terminal_refresh_enabled = false
FROM public.auctions AS auction
WHERE auction.id = sync.auction_id AND auction.status = 'settled';

CREATE INDEX auction_forum_terminal_due_idx
    ON public.auction_forum_sync (content_next_scan_at, auction_id)
    WHERE status = 'published' AND terminal_refresh_enabled
      AND terminal_synced_version IS NULL;
