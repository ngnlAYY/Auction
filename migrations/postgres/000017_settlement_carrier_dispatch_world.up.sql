-- WinnerPaymentCarrierCod 的义务来源世界仍是 isekai，但外部履约契约固定从 main 派发。
-- 该约束修正通过追加迁移完成，不能修改已发布的 000002。
LOCK TABLE public.settlement_obligations, public.settlement_obligation_fulfillments
IN SHARE ROW EXCLUSIVE MODE;

-- 000002 的大形状 CHECK 未命名，名称由 PostgreSQL 按建表顺序生成；按约束定义定位它，
-- 避免把机器上的自动命名当作迁移契约。若基线已被改写，迁移失败关闭。
DO $$
DECLARE
    constraint_name text;
BEGIN
    SELECT catalog_constraint.conname
    INTO constraint_name
    FROM pg_catalog.pg_constraint AS catalog_constraint
    WHERE catalog_constraint.conrelid = 'public.settlement_obligations'::regclass
      AND catalog_constraint.contype = 'c'
      AND pg_get_constraintdef(catalog_constraint.oid) LIKE '%winner_payment_carrier_cod%'
      AND pg_get_constraintdef(catalog_constraint.oid) LIKE '%world_id%'
      AND pg_get_constraintdef(catalog_constraint.oid) LIKE '%isekai%';

    IF constraint_name IS NULL THEN
        RAISE EXCEPTION '未找到 000002 的 settlement obligation 世界约束';
    END IF;

    EXECUTE format(
        'ALTER TABLE public.settlement_obligations DROP CONSTRAINT %I',
        constraint_name
    );
END
$$;

-- 义务的 world_id 是业务来源世界，不能改写为 dispatch world；已有错误写成 main 的
-- carrier 义务也恢复为旧版 isekai 来源，只有履约记录改走 main。
UPDATE public.settlement_obligations
SET world_id = 'isekai',
    updated_at = clock_timestamp()
WHERE obligation_kind = 'winner_payment_carrier_cod'
  AND world_id = 'main';

UPDATE public.settlement_obligation_fulfillments AS fulfillment
SET dispatch_world = 'main',
    updated_at = clock_timestamp()
FROM public.settlement_obligations AS obligation
WHERE obligation.id = fulfillment.obligation_id
  AND obligation.obligation_kind = 'winner_payment_carrier_cod'
  AND fulfillment.dispatch_world = 'isekai';

ALTER TABLE public.settlement_obligations
    ADD CONSTRAINT settlement_obligations_kind_shape_check CHECK (
        (
            obligation_kind = 'winner_equipment_cod'
            AND world_id = 'main'
            AND business_state = 'ready'
            AND recipient_user_id IS NOT NULL
            AND amount > 0
            AND carrier_credit = 0
            AND prerequisite_obligation_id IS NULL
        )
        OR (
            obligation_kind = 'winner_payment_carrier_cod'
            AND world_id = 'isekai'
            AND business_state = 'ready'
            AND recipient_user_id IS NOT NULL
            AND amount > 0
            AND carrier_credit = 1
            AND prerequisite_obligation_id IS NULL
        )
        OR (
            obligation_kind = 'winner_equipment_handoff'
            AND world_id = 'isekai'
            AND business_state IN ('blocked', 'ready')
            AND recipient_user_id IS NOT NULL
            AND amount = 0
            AND carrier_credit = 0
            AND prerequisite_obligation_id IS NOT NULL
        )
        OR (
            obligation_kind = 'seller_payout'
            AND business_state IN ('blocked', 'ready')
            AND recipient_game_identity IS NOT NULL
            AND amount > 0
            AND carrier_credit = 0
            AND prerequisite_obligation_id IS NOT NULL
        )
        OR (
            obligation_kind = 'buyer_refund'
            AND business_state IN ('contingent', 'ready')
            AND recipient_user_id IS NOT NULL
            AND amount > 0
            AND carrier_credit = 0
            AND prerequisite_obligation_id IS NOT NULL
        )
        OR (
            obligation_kind IN ('unsold_return', 'withdrawn_return')
            AND business_state = 'ready'
            AND recipient_game_identity IS NOT NULL
            AND amount = 0
            AND carrier_credit = 0
            AND prerequisite_obligation_id IS NULL
        )
    );
