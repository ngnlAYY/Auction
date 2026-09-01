-- 为每场拍卖保存固定楼层的真实远端身份：#1 为拍卖目录，#2 为系统编辑/错误记录。
-- 历史 published 行保持“尚未对账”状态，Worker 必须读取远端主题后再补齐，不能猜测 #2。
ALTER TABLE public.auction_forum_sync
    ADD COLUMN catalog_post_id text,
    ADD COLUMN system_log_post_id text,
    ADD COLUMN system_log_body text,
    ADD COLUMN content_revision bigint NOT NULL DEFAULT 0,
    ADD COLUMN content_synced_at timestamptz,
    ADD COLUMN content_next_scan_at timestamptz NOT NULL DEFAULT to_timestamp(0),
    ADD COLUMN content_lease_expires_at timestamptz,
    ADD COLUMN content_claim_token uuid,
    ADD COLUMN content_claim_owner text,
    ADD COLUMN content_claim_fencing bigint NOT NULL DEFAULT 0;

ALTER TABLE public.auction_forum_sync
    ADD CONSTRAINT auction_forum_sync_fixed_posts_check CHECK (
        (
            catalog_post_id IS NULL
            AND system_log_post_id IS NULL
            AND system_log_body IS NULL
            AND content_revision = 0
            AND content_synced_at IS NULL
        )
        OR (
            catalog_post_id IS NOT NULL
            AND catalog_post_id ~ '^[1-9][0-9]*$'
            AND octet_length(catalog_post_id) <= 32
            AND system_log_post_id IS NOT NULL
            AND system_log_post_id ~ '^[1-9][0-9]*$'
            AND octet_length(system_log_post_id) <= 32
            AND catalog_post_id <> system_log_post_id
            AND system_log_body IS NOT NULL
            AND octet_length(system_log_body) BETWEEN 1 AND 65531
            AND content_revision > 0
            AND content_synced_at IS NOT NULL
        )
    ),
    ADD CONSTRAINT auction_forum_sync_content_claim_check CHECK (
        content_claim_fencing >= 0
        AND (
            (
                content_claim_token IS NULL
                AND content_claim_owner IS NULL
                AND content_lease_expires_at IS NULL
            )
            OR (
                content_claim_token IS NOT NULL
                AND content_claim_owner IS NOT NULL
                AND content_claim_owner = btrim(content_claim_owner)
                AND content_claim_owner <> ''
                AND octet_length(content_claim_owner) <= 256
                AND content_lease_expires_at IS NOT NULL
            )
        )
    );

CREATE INDEX auction_forum_sync_content_due_idx
    ON public.auction_forum_sync (content_next_scan_at, auction_id)
    WHERE status = 'published';

COMMENT ON COLUMN public.auction_forum_sync.catalog_post_id IS
    '远端固定 #1 拍卖目录 post ID；未对账的历史主题保持 NULL';
COMMENT ON COLUMN public.auction_forum_sync.system_log_post_id IS
    '远端固定 #2 系统编辑/错误记录 post ID；不得从楼层位置猜测';
COMMENT ON COLUMN public.auction_forum_sync.system_log_body IS
    '最近一次得到 exact post proof 的固定 #2 规范正文';
COMMENT ON COLUMN public.auction_forum_sync.content_revision IS
    '固定 #1/#2 正文证明的本地单调版本';
COMMENT ON COLUMN public.auction_forum_sync.content_next_scan_at IS
    '固定楼层下一次可领取扫描水位；与主题发布 retry_at 分离';
COMMENT ON COLUMN public.auction_forum_sync.content_lease_expires_at IS
    '固定楼层刷新内容租约截止时间；远端编辑完成前不得并发领取';
COMMENT ON COLUMN public.auction_forum_sync.content_claim_token IS
    '固定楼层刷新 claim 的随机身份；与 owner/fencing/lease 共同校验';
COMMENT ON COLUMN public.auction_forum_sync.content_claim_owner IS
    '持有固定楼层刷新 claim 的 Worker 实例身份';
COMMENT ON COLUMN public.auction_forum_sync.content_claim_fencing IS
    '固定楼层刷新 claim 的单调 fencing；远端写入前由单一 Worker 持有独立内容租约';
