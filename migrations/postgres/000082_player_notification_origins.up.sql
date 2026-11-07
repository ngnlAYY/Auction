-- storage owner：W14.3 新增两类玩家通知（卖方成交汇总、换季前 isekai 领取/交付提醒）。
--
-- 不变量：
--   * 通知只追加；正文、收件人与冻结赛季边界在创建时固定，之后只允许在 sending 围栏前退役。
--   * 每场每卖方最多一封汇总，每个冻结赛季边界每个收件人最多一封提醒；由部分唯一索引强制。
--   * 既有 origin 闭集、结算载荷、冻结策略与历史投递都不改写；只放宽 CHECK 接受两个新来源。
--   * 新来源不携带资产，仍由 delivery owner 统一发送与审计。
--
-- 兼容等级 breaking：旧程序不认识 seller_summary/season_reminder，领取后会按未知来源
-- 失败关闭。部署须在停机门禁下与同版程序一起切换；回退旧程序前须先处理或等待这两类
-- 通知全部终态（同一回退要求见 ops/deploy/RECOVERY.md）。
--
-- 顺序：新来源行只由新程序写入，因此本迁移与同版程序同时安装即可，不要求旧 Worker 先停。

ALTER TABLE public.deliveries DROP CONSTRAINT deliveries_origin_kind_check;
ALTER TABLE public.deliveries ADD CONSTRAINT deliveries_origin_kind_check CHECK (
    origin_kind IN (
        'settlement',
        'settlement_recovery_action',
        'user_key',
        'manual',
        'mail_intake',
        'operations_alert',
        'lot_watch',
        'seller_summary',
        'season_reminder'
    )
);

-- 两类新通知都属于结算后的结果沟通；保持结算类发送节奏，不占用出价提醒优先级。
CREATE OR REPLACE FUNCTION public.delivery_send_category(origin text, payload jsonb)
RETURNS text LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE
        WHEN payload ->> 'operation' IS DISTINCT FROM 'send' THEN NULL
        WHEN origin = 'lot_watch' THEN 'outbid'
        WHEN origin = 'operations_alert' THEN 'operations'
        WHEN origin IN ('seller_summary', 'season_reminder') THEN 'settlement'
        WHEN origin = 'mail_intake' AND payload ->> 'template'
            IN ('register_key', 'register_result', 'rebind_result', 'user_assets_rejected')
            THEN 'account'
        WHEN jsonb_typeof(payload -> 'equipment_secret_refs') = 'array'
             AND payload -> 'equipment_secret_refs' <> '[]'::jsonb THEN 'equipment'
        WHEN jsonb_typeof(payload -> 'attachment_ids') = 'array'
             AND payload -> 'attachment_ids' <> '[]'::jsonb THEN 'equipment'
        WHEN origin IN ('settlement', 'settlement_recovery_action') THEN 'settlement'
        WHEN origin = 'mail_intake' THEN 'intake'
        ELSE 'manual'
    END
$$;

-- 同一身份只允许一条投递：卖方汇总按 (auction, seller)，换季提醒按 (冻结边界, recipient)。
-- 身份字符串由应用层按同一规则生成；退役后也不重建，避免重复打扰。
CREATE UNIQUE INDEX deliveries_player_notification_identity_idx
    ON public.deliveries (origin_kind, origin_id)
    WHERE origin_kind IN ('seller_summary', 'season_reminder');

-- 卖方汇总只扫描最近结算的场次；部分索引避免全表扫描历史结算。
CREATE INDEX auctions_settled_at_idx
    ON public.auctions (settled_at, id)
    WHERE status = 'settled';

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000082', 'system.migration', 'migration',
    '000082', 'success', 'system',
    jsonb_build_object(
        'summary', 'player notification origins and identities',
        'compatibility', 'breaking'
    )
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (82, 'breaking');
