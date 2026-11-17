-- 已付款交付被收回后的成交撤销收口：`delivery_dispositions` 的种类闭集新增
-- `recovery_equipment_recovered`。
-- owner: auction-domain（处置种类/来源闭集）、auction-storage（恢复确认写入、告警与对账读侧）
--
-- 背景：isekai 买家已付载体 COD，但交付装备被系统退回、装备经精确 Take 证明收回后，Admin 确认
--   `equipment_recovered`。确认事务此前只激活买方退款、重列装备，从未开始的卖方货款与已寄出的
--   交付仍停在未终结状态；运行期改为在同一事务内把二者收口为 `failed`（`recovery_sale_voided` /
--   `recovery_equipment_recovered`），并在原交付投递当前 fence 上追加本种类的处置事实。它不是
--   COD 退回，不能借用 `cod_returned`。
--
-- 不变量：
--   1. 只放宽处置表的种类闭集与配套约束，不改写任何既有行，不改变投递、义务、恢复、资金或资产。
--   2. 新种类恰好一个来源：`settlement_recovery_id`（沿用 000100 的来源列与“恰好一个来源”约束）。
--   3. 新种类写入时固定 `resolves_alert = true`；读侧只认同一恢复记录的卖方货款与交付义务。
--   4. 处置表仍只追加（000088 的触发器不变）；运行账号权限不变。
--
-- 等级：breaking。放宽 CHECK 后会写入旧程序无法解析的处置种类（旧领域闭集解析失败、旧告警与
--   对账口径把这些收口计为未处置）；回退须按 RECOVERY.md 处理含 breaking 迁移的版本。
LOCK TABLE public.delivery_dispositions IN SHARE ROW EXCLUSIVE MODE;

ALTER TABLE public.delivery_dispositions
    DROP CONSTRAINT delivery_dispositions_kind_check,
    DROP CONSTRAINT delivery_dispositions_check2,
    DROP CONSTRAINT delivery_dispositions_check7,
    ADD CONSTRAINT delivery_dispositions_kind_check CHECK (
        kind IN (
            'auto_sent_proof', 'operator_sent_confirmation', 'admin_failed',
            'equipment_loss', 'cod_returned', 'recovery_equipment_recovered', 'legacy_self',
            'preflight_retired', 'operator_acknowledged'
        )
    ),
    ADD CONSTRAINT delivery_dispositions_check2 CHECK (
        (kind = 'auto_sent_proof' AND sent_binding_world_id IS NOT NULL)
        OR (kind = 'operator_sent_confirmation' AND sent_confirmation_id IS NOT NULL)
        OR (kind = 'admin_failed' AND admin_command_id IS NOT NULL)
        OR (kind = 'equipment_loss' AND equipment_loss_id IS NOT NULL)
        OR (kind = 'cod_returned' AND settlement_recovery_id IS NOT NULL)
        OR (kind = 'recovery_equipment_recovered' AND settlement_recovery_id IS NOT NULL)
        OR (kind = 'legacy_self' AND legacy_self_reconciliation_id IS NOT NULL)
        OR (kind = 'operator_acknowledged' AND source_audit_id IS NOT NULL)
        OR kind = 'preflight_retired'
    ),
    ADD CONSTRAINT delivery_dispositions_check7 CHECK (
        (kind IN ('auto_sent_proof', 'operator_sent_confirmation', 'admin_failed',
                  'equipment_loss', 'cod_returned', 'recovery_equipment_recovered',
                  'legacy_self', 'operator_acknowledged') AND resolves_alert)
        OR (kind = 'preflight_retired'
            AND resolves_alert = (reason_code = 'lot_watch_invalidated'))
    );

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000113', 'system.migration', 'migration',
    '000113', 'success', 'system',
    jsonb_build_object(
        'summary', 'recovery equipment recovered disposition',
        'compatibility', 'breaking')
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (113, 'breaking');
