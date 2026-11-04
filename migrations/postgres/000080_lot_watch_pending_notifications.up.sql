-- W14.11：慢速代理等待反击期间的被超提醒只记待定事实，持有人能行动时才入队。
-- 不变量：待定事实只追加行，状态只从 pending 单向进入 enqueued/retired，
-- 每个 lot × 用户 × 领先周期最多一条；竞价结果、代理上限和已冻结投递载荷都不改写。
CREATE TABLE public.lot_watch_pending_notifications (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE RESTRICT,
    lot_id uuid NOT NULL REFERENCES public.lots(id) ON DELETE RESTRICT,
    leadership_cycle bigint NOT NULL CHECK (leadership_cycle >= 0),
    displaced_bid_id uuid NOT NULL REFERENCES public.bids(id) ON DELETE RESTRICT,
    watch_version bigint NOT NULL CHECK (watch_version > 0),
    preference_version bigint NOT NULL CHECK (preference_version > 0),
    channel text NOT NULL CHECK (channel IN ('mm', 'pm')),
    -- mm 保存冻结的用户名；pm 保存论坛账号数字键，缺失时为 NULL（发送时 suppressed）。
    recipient text,
    amount bigint NOT NULL CHECK (amount > 0),
    status text NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'enqueued', 'retired')),
    created_at timestamptz NOT NULL,
    resolved_at timestamptz,
    resolution_reason text CHECK (
        resolution_reason IS NULL
        OR resolution_reason IN (
            'slow_proxy_exhausted',
            'leading_again',
            'lot_closed',
            'watch_removed'
        )
    ),
    UNIQUE (lot_id, user_id, leadership_cycle),
    CHECK ((status = 'pending') = (resolved_at IS NULL)),
    CHECK ((status = 'pending') = (resolution_reason IS NULL))
);

CREATE INDEX lot_watch_pending_notifications_due_idx
    ON public.lot_watch_pending_notifications (lot_id, created_at)
    WHERE status = 'pending';

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000080', 'system.migration', 'migration',
    '000080', 'success', 'system',
    jsonb_build_object('summary', 'lot watch pending notifications', 'compatibility', 'compatible')
);

-- 纯新增表与索引；旧程序不读该表，因此登记 compatible。
INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (80, 'compatible');
