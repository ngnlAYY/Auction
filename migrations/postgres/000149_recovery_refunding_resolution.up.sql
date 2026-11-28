-- `refunding` 阶段的恢复必须已有明确的 `equipment_recovered` 结论。
-- owner: auction-storage
--
-- 不变量：
--   1. 000005 的 `CHECK (stage <> 'refunding' OR (world_id = 'isekai' AND resolution =
--      'equipment_recovered'))` 在 `resolution IS NULL` 时求值为 NULL，CHECK 不拒绝。
--   2. `settlement_recoveries` 的 `refunding` 只能由完整装备收回的恢复进入。
--   3. 约束以 `NOT VALID` 加入，既有行保持原样，新写入立即受约束。
--
-- 等级：breaking。新增会拒绝旧程序可能写入的无结论 `refunding` 行，须与同版程序一起切换。

ALTER TABLE public.settlement_recoveries
    ADD CONSTRAINT settlement_recoveries_refunding_resolution_000149 CHECK (
        stage <> 'refunding'
        OR resolution IS NOT DISTINCT FROM 'equipment_recovered'
    ) NOT VALID;

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (149, 'breaking');
