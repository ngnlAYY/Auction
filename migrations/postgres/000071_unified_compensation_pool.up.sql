-- settlement owner：把按世界拆分的补偿池合并为一个以 main Credits 计的全局补偿池。
-- 背景：isekai 成交的买方付款（main 载体 COD）与卖方货款都在 main 以 Credits 结清，附加费与
-- 补偿实际进出的都是 main Credits；按世界分池只是把同一种资金拆成两本账。
-- 不变量：
--   1. 全局池期初余额 = 合并时各世界池余额之和，资金总额不变；
--   2. 全局池 state_version 从各世界池最大版本 + 1 起算，任何一条旧链上的版本都继续单调递增；
--   3. settlement_pools 冻结为合并时各世界池的期末证据（禁止 INSERT/UPDATE/DELETE），合并本身
--      追加一条系统审计；已写入的结算结果、补偿账本与冲正不改写；
--   4. 池的范围不属于场次冻结策略：结算公式与锚点不变，期初余额本就是结算时在锁内读取的运行时
--      输入，因此已创建但尚未结算的场次在合并后直接使用全局池，合并前结算的成交撤销也冲正到全局池。
-- 升级前须停止全部旧角色：旧二进制仍写 settlement_pools，会被冻结触发器拒绝并失败关闭。
LOCK TABLE public.settlement_pools IN SHARE ROW EXCLUSIVE MODE;

CREATE TABLE public.compensation_pool (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    balance bigint NOT NULL CHECK (balance >= 0),
    state_version bigint NOT NULL CHECK (state_version > 0),
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CHECK (updated_at >= created_at)
);

-- 合计超出 bigint 时转换失败并回滚整个迁移，不截断余额。
INSERT INTO public.compensation_pool (singleton, balance, state_version)
SELECT true,
       COALESCE(sum(pool.balance), 0)::bigint,
       COALESCE(max(pool.state_version), 0) + 1
FROM public.settlement_pools AS pool;

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
SELECT gen_random_uuid(), 'system', 'migration-000071', 'auction.compensation.merge',
       'compensation_pool', 'global', 'success', 'system',
       jsonb_build_object(
           'migration_version', 71,
           'world_pools', (
               SELECT COALESCE(
                   jsonb_agg(
                       jsonb_build_object(
                           'world_id', world_pool.world_id,
                           'balance', world_pool.balance,
                           'state_version', world_pool.state_version,
                           'updated_at', world_pool.updated_at
                       )
                       ORDER BY world_pool.world_id
                   ),
                   '[]'::jsonb
               )
               FROM public.settlement_pools AS world_pool
           ),
           'balance', pool.balance,
           'state_version', pool.state_version
       )
FROM public.compensation_pool AS pool;

CREATE FUNCTION public.reject_merged_world_pool_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'settlement_pools were merged into compensation_pool and are frozen'
        USING ERRCODE = '55000';
END;
$$;

CREATE TRIGGER settlement_pools_frozen
BEFORE INSERT OR UPDATE OR DELETE ON public.settlement_pools
FOR EACH ROW EXECUTE FUNCTION public.reject_merged_world_pool_mutation();
