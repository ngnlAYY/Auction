-- 场次结算池余额必须完整满足开账 + 税 - 补偿的等式，而不是只有上界。
-- owner: auction-storage
--
-- 不变量：
--   1. 000002 只校验 `closing_pool_balance <= opening_pool_balance + tax_total`；
--      领域计划要求启用补偿池时 `closing = opening + tax - compensation`，禁用时 `closing = opening`
--      且 `compensation_total = 0`。
--   2. 一致性检查器按同一等式报告 `global_pool_movement_chain_mismatch`；本约束把新写入也固定在
--      同一口径。
--   3. 既有行可能包含历史自对账形状，约束以 `NOT VALID` 加入：历史行保持原样，新写入立即受约束。
--
-- 等级：breaking。新增会拒绝旧程序可能写入的余额关系，须与同版程序在停机门禁下切换。

ALTER TABLE public.auction_settlements
    ADD CONSTRAINT auction_settlements_pool_balance_equation_000146 CHECK (
        closing_pool_balance::numeric = opening_pool_balance::numeric
            + CASE
                WHEN pool_enabled THEN tax_total::numeric - compensation_total::numeric
                ELSE 0
            END
    ) NOT VALID;

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (146, 'breaking');
