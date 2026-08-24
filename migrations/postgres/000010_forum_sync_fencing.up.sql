-- 论坛发布必须使用可持久化的 token、owner 和 fencing，避免租约过期的旧 Worker 覆盖新结果。
ALTER TABLE public.auction_forum_sync
    ADD COLUMN claim_token uuid,
    ADD COLUMN claim_owner text,
    ADD COLUMN claim_fencing bigint NOT NULL DEFAULT 0
        CHECK (claim_fencing >= 0),
    ADD CONSTRAINT auction_forum_sync_claim_shape_check CHECK (
        (claim_token IS NULL AND claim_owner IS NULL)
        OR (claim_token IS NOT NULL AND claim_owner IS NOT NULL
            AND claim_owner = btrim(claim_owner) AND claim_owner <> '')
    );

CREATE INDEX auction_forum_sync_claim_fence_idx
    ON public.auction_forum_sync (claim_token, claim_owner, claim_fencing)
    WHERE claim_token IS NOT NULL;
