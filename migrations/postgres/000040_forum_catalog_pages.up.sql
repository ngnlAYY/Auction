-- 目录每帖最多 150 件，系统日志紧接最后一页；历史主题不猜测续页或移动既有回帖。
ALTER TABLE public.auction_forum_sync
    ADD COLUMN catalog_layout text NOT NULL DEFAULT 'legacy_single'
        CHECK (catalog_layout IN ('legacy_single', 'paged_v1')),
    ADD COLUMN catalog_page_count integer NOT NULL DEFAULT 1
        CHECK (catalog_page_count BETWEEN 1 AND 67),
    ADD CONSTRAINT auction_forum_sync_catalog_floor_check
        CHECK (processed_through_post_number > catalog_page_count),
    ADD CONSTRAINT auction_forum_sync_legacy_catalog_check
        CHECK (catalog_layout <> 'legacy_single' OR catalog_page_count = 1);

-- 只为从未领取/尝试且无远端身份的队列项启用分页；未知 POST 必须沿用原始单页负载对账。
UPDATE public.auction_forum_sync
SET catalog_layout = 'paged_v1'
WHERE status = 'pending' AND attempt_count = 0
  AND claim_token IS NULL AND thread_id IS NULL
  AND catalog_post_id IS NULL AND system_log_post_id IS NULL;
ALTER TABLE public.auction_forum_sync ALTER COLUMN catalog_layout SET DEFAULT 'paged_v1';

-- 这里只保存非权威论坛镜像。发布与刷新由 forum owner 锁定主记录后同事务写入；
-- 领取时必须验证行数、连续楼层和所有 post ID，缺页或身份冲突失败关闭。
CREATE TABLE public.auction_forum_catalog_pages (
    auction_id uuid NOT NULL REFERENCES public.auction_forum_sync(auction_id) ON DELETE CASCADE,
    post_number integer NOT NULL CHECK (post_number BETWEEN 2 AND 67),
    post_id text NOT NULL CHECK (post_id ~ '^[1-9][0-9]*$' AND octet_length(post_id) <= 32),
    body text NOT NULL CHECK (octet_length(body) BETWEEN 1 AND 65531),
    PRIMARY KEY (auction_id, post_number),
    UNIQUE (auction_id, post_id)
);

COMMENT ON COLUMN public.auction_forum_sync.catalog_layout IS
    '首次发布前冻结：历史已尝试主题保持 legacy_single，未尝试/新主题使用每帖150件的 paged_v1';
COMMENT ON COLUMN public.auction_forum_sync.catalog_page_count IS
    '已证明的连续目录页数；系统日志楼层为此值加一，历史主题默认一页且不自动改楼';
COMMENT ON TABLE public.auction_forum_catalog_pages IS
    '从 #2 开始的目录续页身份和正文证明；与首帖及日志同事务发布、同一内容租约和 revision CAS 刷新';
COMMENT ON COLUMN public.auction_forum_catalog_pages.post_number IS
    '完整主题读取证明的连续目录楼层，不由 post ID 推算';
COMMENT ON COLUMN public.auction_forum_catalog_pages.post_id IS
    '经 exact post 回执及拍卖分页标记证明的远端稳定身份';
COMMENT ON COLUMN public.auction_forum_catalog_pages.body IS
    '最近一次已证明发送的续页正文；逐字一致性取决于部署的正文回读校验策略';
COMMENT ON COLUMN public.auction_forum_sync.system_log_post_id IS
    '最后一个目录页之后的系统更新帖子真实 post ID；不得从楼层位置猜测';
COMMENT ON COLUMN public.auction_forum_sync.system_log_body IS
    '最近一次得到 exact post proof 的系统更新楼正文';
COMMENT ON COLUMN public.auction_forum_sync.content_revision IS
    '所有目录页与系统更新楼正文证明的本地单调版本';
COMMENT ON COLUMN public.auction_forum_sync.processed_through_post_number IS
    '完整主题读取证明已处理到的连续真实楼层；从系统日志楼层安全起步';
