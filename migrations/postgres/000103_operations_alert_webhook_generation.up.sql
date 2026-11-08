-- W5.1：webhook 告警按活动周期隔离未决投递，避免上一周期的 manual_reconcile 永久静音同类告警。
-- owner: auction-application（周期语义）、auction-storage（signal/投递账本）
--
-- 不变量：
--   1. webhook_generation 只在 signal 从 inactive 变为 active 时递增；活动周期内的重复观察不换周期。
--   2. 同一 fingerprint 在同一 generation 内最多一个 pending/sending/manual_reconcile 投递；
--      新周期可以有独立投递，但旧周期的未知结果仍保持 manual_reconcile。
--   3. 旧程序忽略新列；旧唯一索引行为被新索引覆盖，回退时仍不会重发未知结果。
--
-- 等级：compatible。仅新增带默认值的列并替换唯一索引；旧程序无需读写新列即可运行。
LOCK TABLE public.operations_alert_signals IN SHARE ROW EXCLUSIVE MODE;
LOCK TABLE public.operations_alert_webhook_deliveries IN SHARE ROW EXCLUSIVE MODE;

ALTER TABLE public.operations_alert_signals
    ADD COLUMN webhook_generation bigint NOT NULL DEFAULT 1
        CHECK (webhook_generation > 0);

ALTER TABLE public.operations_alert_webhook_deliveries
    ADD COLUMN signal_generation bigint NOT NULL DEFAULT 1
        CHECK (signal_generation > 0);

DROP INDEX public.operations_alert_webhook_open_fingerprint_idx;

CREATE UNIQUE INDEX operations_alert_webhook_open_fingerprint_idx
    ON public.operations_alert_webhook_deliveries (fingerprint, signal_generation)
    WHERE status IN ('pending', 'sending', 'manual_reconcile');

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000103', 'system.migration', 'migration',
    '000103', 'success', 'system',
    jsonb_build_object('summary', 'operations alert webhook incident generation', 'compatibility', 'compatible')
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (103, 'compatible');
