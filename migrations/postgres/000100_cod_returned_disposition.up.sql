-- W2.6：COD 三次退回终态使用统一处置事实收口。
-- owner: auction-domain（处置种类/来源闭集）、auction-storage（写入、读取与 SQL）
--
-- 不变量：
--   1. 只追加 `delivery_dispositions` 的一种新事实，不改变投递、义务、恢复、资金或资产状态。
--   2. `cod_returned` 绑定原 COD 投递（delivery + fencing），来源引用恢复记录
--      `settlement_recovery_id`；恰好一个来源，仍由 CHECK 强制。
--   3. 处置事实与 COD 退回终态的履约失败在同一事务提交；处置缺失时读侧继续告警。
--   4. 读侧只在恢复记录的 payment/seller payout/buyer refund 义务上解除失败计数，
--      不跨恢复记录、不按金额或名称猜测。
-- 等级：breaking。新增 kind 与来源列，旧程序无法解析该 kind；回退须按
-- RECOVERY.md 处理含 breaking 迁移的版本。
LOCK TABLE public.delivery_dispositions IN SHARE ROW EXCLUSIVE MODE;

ALTER TABLE public.delivery_dispositions
    ADD COLUMN settlement_recovery_id uuid
        REFERENCES public.settlement_recoveries(id) ON DELETE RESTRICT,
    DROP CONSTRAINT delivery_dispositions_kind_check,
    DROP CONSTRAINT delivery_dispositions_check1,
    DROP CONSTRAINT delivery_dispositions_check2,
    DROP CONSTRAINT delivery_dispositions_check7,
    ADD CONSTRAINT delivery_dispositions_kind_check CHECK (
        kind IN (
            'auto_sent_proof', 'operator_sent_confirmation', 'admin_failed',
            'equipment_loss', 'cod_returned', 'legacy_self', 'preflight_retired',
            'operator_acknowledged'
        )
    ),
    ADD CONSTRAINT delivery_dispositions_check1 CHECK (
        num_nonnulls(sent_confirmation_id, admin_command_id, equipment_loss_id,
                     legacy_self_reconciliation_id, sent_binding_world_id,
                     settlement_recovery_id, source_audit_id)
        = CASE WHEN kind = 'preflight_retired' THEN 0 ELSE 1 END
    ),
    ADD CONSTRAINT delivery_dispositions_check2 CHECK (
        (kind = 'auto_sent_proof' AND sent_binding_world_id IS NOT NULL)
        OR (kind = 'operator_sent_confirmation' AND sent_confirmation_id IS NOT NULL)
        OR (kind = 'admin_failed' AND admin_command_id IS NOT NULL)
        OR (kind = 'equipment_loss' AND equipment_loss_id IS NOT NULL)
        OR (kind = 'cod_returned' AND settlement_recovery_id IS NOT NULL)
        OR (kind = 'legacy_self' AND legacy_self_reconciliation_id IS NOT NULL)
        OR (kind = 'operator_acknowledged' AND source_audit_id IS NOT NULL)
        OR kind = 'preflight_retired'
    ),
    ADD CONSTRAINT delivery_dispositions_check7 CHECK (
        (kind IN ('auto_sent_proof', 'operator_sent_confirmation', 'admin_failed',
                  'equipment_loss', 'cod_returned', 'legacy_self',
                  'operator_acknowledged') AND resolves_alert)
        OR (kind = 'preflight_retired'
            AND resolves_alert = (reason_code = 'lot_watch_invalidated'))
    );

CREATE INDEX delivery_dispositions_recovery_idx
    ON public.delivery_dispositions (settlement_recovery_id)
    WHERE settlement_recovery_id IS NOT NULL;

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000100', 'system.migration', 'migration',
    '000100', 'success', 'system',
    jsonb_build_object(
        'summary', 'cod returned terminal disposition',
        'compatibility', 'breaking')
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (100, 'breaking');
