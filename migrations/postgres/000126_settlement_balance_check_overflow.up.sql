-- 结算 owner：用 NUMERIC 中间值比较 bigint 补偿池余额，避免检查表达式本身溢出。
-- owner: auction-storage
-- 不变量：closing_pool_balance 不超过 opening_pool_balance 与 tax_total 的数学和。
-- Breaking：替换既有 CHECK；升级前停止所有写入角色，旧 CHECK 名称不符时失败关闭。
LOCK TABLE public.auction_settlements IN SHARE ROW EXCLUSIVE MODE;

DO $block$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_catalog.pg_constraint AS constraint_record
        WHERE constraint_record.conrelid = 'public.auction_settlements'::regclass
          AND constraint_record.conname = 'auction_settlements_check1'
          AND constraint_record.contype = 'c'
          AND constraint_record.convalidated
    ) THEN
        RAISE EXCEPTION '预期的结算池 bigint 余额约束缺失或未验证'
            USING ERRCODE = '23514';
    END IF;
END
$block$;

ALTER TABLE public.auction_settlements
    DROP CONSTRAINT auction_settlements_check1,
    ADD CONSTRAINT auction_settlements_closing_pool_balance_upper_bound_check
        CHECK (
            closing_pool_balance::numeric
                <= opening_pool_balance::numeric + tax_total::numeric
        );

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000126', 'system.migration', 'migration',
    '000126', 'success', 'system',
    jsonb_build_object(
        'summary', 'overflow-safe settlement pool balance constraint',
        'compatibility', 'breaking')
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (126, 'breaking');
