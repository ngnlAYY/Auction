-- 为固定 #1/#2 扫描增加持久化健康证据；调度 lease 与 Public 新鲜度保持分离。
ALTER TABLE public.auction_forum_sync
    ADD COLUMN content_last_success_at timestamptz,
    ADD COLUMN content_last_error_at timestamptz,
    ADD COLUMN content_last_error_code text,
    ADD CONSTRAINT auction_forum_sync_content_error_pair_check CHECK (
        (content_last_error_at IS NULL) = (content_last_error_code IS NULL)
    ),
    ADD CONSTRAINT auction_forum_sync_content_error_code_check CHECK (
        content_last_error_code IS NULL
        OR (
            content_last_error_code <> ''
            AND length(content_last_error_code) <= 64
            AND content_last_error_code = btrim(content_last_error_code)
        )
    );

-- 自动同步论坛数字账号和可变玩家名称时，历史必须追加且不能复用可撤销的 Admin rename 表。
CREATE TABLE public.forum_identity_history (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE RESTRICT,
    world_id text NOT NULL CHECK (world_id IN ('main', 'isekai')),
    provider text NOT NULL CHECK (provider = 'ehentai'),
    external_account_id text NOT NULL CHECK (external_account_id <> ''),
    previous_username text NOT NULL CHECK (previous_username <> ''),
    next_username text NOT NULL CHECK (next_username <> ''),
    previous_forum_account_id text,
    next_forum_account_id text NOT NULL CHECK (next_forum_account_id <> ''),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CHECK (
        previous_username IS DISTINCT FROM next_username
        OR previous_forum_account_id IS DISTINCT FROM next_forum_account_id
    )
);

CREATE INDEX forum_identity_history_user_idx
    ON public.forum_identity_history (user_id, created_at DESC);
