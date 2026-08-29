-- 论坛主题 POST 结果未知时必须先按精确标题只读对账；只有人工命令可以重新授权远端写入。
-- 命令账本、状态推进和 audit_records 在同一数据库事务提交，重复 command_id 只能重放原结果。
CREATE TABLE public.forum_sync_admin_commands (
    command_id uuid PRIMARY KEY,
    command_kind text NOT NULL CHECK (command_kind = 'authorize_write_retry'),
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    auction_id uuid NOT NULL REFERENCES public.auctions(id) ON DELETE RESTRICT,
    actor_id text NOT NULL CHECK (
        octet_length(actor_id) BETWEEN 1 AND 256
        AND actor_id = btrim(actor_id)
        AND actor_id !~ '[[:cntrl:]]'
    ),
    result_payload jsonb NOT NULL CHECK (jsonb_typeof(result_payload) = 'object'),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp()
);

CREATE INDEX forum_sync_admin_commands_auction_idx
    ON public.forum_sync_admin_commands (auction_id, created_at, command_id);
