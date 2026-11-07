-- 修复 `rekey` 回信的发送类别：082 重定义 `delivery_send_category` 时漏回 079 已加入的
-- `rekey_key`/`rekey_result`，导致登录 key 轮换回信落入 `intake`，违反决策 61 的账号回信优先级。
-- owner: auction-storage（`deliveries` 与发送类别函数的唯一 SQL owner）
--
-- 不变量：
--   1. 只改变发送类别投影；投递载荷、状态、租约、sending 围栏、资金与审计事实不变。
--   2. 分类顺序与 082 完全一致，只在 `account` 模板闭集中补回 `rekey_key`/`rekey_result`。
--   3. `deliveries_admin_category_idx` 是表达式索引；函数语义改变后必须重建索引，否则按
--      `account` 查询会漏掉仍以旧分类入索引的既有行。REINDEX 只重建派生索引，不改写业务行。
--   4. 旧程序读取同一函数也只会得到更准确的类别，不涉及无法解析的新状态或列。
--
-- 等级：breaking。改变既有不可变函数语义并重建表达式索引，不能作为纯新增兼容。
LOCK TABLE public.deliveries IN SHARE ROW EXCLUSIVE MODE;

CREATE OR REPLACE FUNCTION public.delivery_send_category(origin text, payload jsonb)
RETURNS text LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE
        WHEN payload ->> 'operation' IS DISTINCT FROM 'send' THEN NULL
        WHEN origin = 'lot_watch' THEN 'outbid'
        WHEN origin = 'operations_alert' THEN 'operations'
        WHEN origin IN ('seller_summary', 'season_reminder') THEN 'settlement'
        WHEN origin = 'mail_intake' AND payload ->> 'template'
            IN (
                'register_key', 'register_result', 'rekey_key', 'rekey_result',
                'rebind_result', 'user_assets_rejected'
            )
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

REINDEX INDEX public.deliveries_admin_category_idx;

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000098', 'system.migration', 'migration',
    '000098', 'success', 'system',
    jsonb_build_object(
        'summary', 'restore rekey templates to account delivery category',
        'compatibility', 'breaking'
    )
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (98, 'breaking');
