-- 装备诊断在 snapshot_invalid/filter_rejected 类别下必须带原因码。
-- owner: auction-storage
--
-- 不变量：
--   1. 000056 的分支写作 `detail_code IN (...)`；`detail_code` 为 JSON null 时整个分支求值为
--      NULL，CHECK 不拒绝，于是两类诊断可以没有原因码。
--   2. 应用端口 `MailEquipmentDiagnosticSnapshot::validate` 与 Admin 读侧已把这两类视为必须携带
--      闭集原因码；补充约束把数据库写入面拉回同一口径。
--   3. 约束以 `NOT VALID` 加入，既有行保持原样，新写入立即受约束。
--
-- 等级：breaking。新增会拒绝旧程序可能写入的无原因码诊断，须与同版程序一起切换。

ALTER TABLE public.inbound_mail
    ADD CONSTRAINT inbound_mail_equipment_diagnostic_detail_required_000151 CHECK (
        equipment_diagnostic IS NULL
        OR jsonb_typeof(equipment_diagnostic -> 'detail_code') = 'string'
        OR equipment_diagnostic ->> 'category' NOT IN ('snapshot_invalid', 'filter_rejected')
    ) NOT VALID;

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (151, 'breaking');
