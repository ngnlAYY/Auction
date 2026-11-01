-- settlement owner：成交撤销后的补偿池冲正事实，以及补偿池公开证据的只追加保护。
-- 不变量：
--   1. 每个恢复记录最多一条冲正，与 resolution 首次落定、补偿池余额/版本变化同事务写入；
--   2. 先退回名义补偿，再冲回附加费；附加费冲回以当时余额为上限，余额永不为负，
--      未冲回部分只在余额耗尽时出现并公开记录；
--   3. 冲正不改写 lot_settlements、auction_settlements、compensation_ledger 或冻结策略；
--      历史已撤销的成交不在本迁移中补冲，避免无证据地改变现有池余额。
-- 升级前须停止全部旧角色；旧 Worker 不会写冲正，若与新版混跑会留下未冲正的撤销。
CREATE TABLE public.compensation_pool_reversals (
    id uuid PRIMARY KEY,
    recovery_id uuid NOT NULL UNIQUE
        REFERENCES public.settlement_recoveries(id) ON DELETE RESTRICT,
    world_id text NOT NULL REFERENCES public.worlds(id) ON DELETE RESTRICT,
    auction_id uuid NOT NULL REFERENCES public.auction_settlements(auction_id) ON DELETE RESTRICT,
    lot_id uuid NOT NULL REFERENCES public.lot_settlements(lot_id) ON DELETE RESTRICT,
    rule_version integer NOT NULL CHECK (rule_version = 1),
    resolution text NOT NULL CHECK (
        resolution IN (
            'reclaimed',
            'cancelled',
            'equipment_recovered',
            'returned_after_three_cod_returns'
        )
    ),
    tax_amount bigint NOT NULL CHECK (tax_amount >= 0),
    compensation_amount bigint NOT NULL CHECK (compensation_amount >= 0),
    tax_reversed bigint NOT NULL CHECK (tax_reversed >= 0),
    opening_balance bigint NOT NULL CHECK (opening_balance >= 0),
    closing_balance bigint NOT NULL CHECK (closing_balance >= 0),
    opening_pool_version bigint NOT NULL CHECK (opening_pool_version > 0),
    closing_pool_version bigint NOT NULL,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (lot_id),
    FOREIGN KEY (auction_id, lot_id) REFERENCES public.lots (auction_id, id) ON DELETE RESTRICT,
    CHECK (tax_amount > 0 OR compensation_amount > 0),
    CHECK (tax_reversed <= tax_amount),
    CHECK (closing_pool_version = opening_pool_version + 1),
    CHECK (closing_balance::numeric
        = opening_balance::numeric + compensation_amount::numeric - tax_reversed::numeric),
    CHECK (tax_reversed = tax_amount OR closing_balance = 0)
);

CREATE INDEX compensation_pool_reversals_auction_idx
    ON public.compensation_pool_reversals (auction_id, lot_id);

CREATE FUNCTION public.reject_compensation_fact_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'settlement and compensation pool facts are append-only'
        USING ERRCODE = '55000';
END;
$$;

-- 结算结果与补偿账本自写入后从未被任何 owner 修改；补上触发器，让公开页展示的证据
-- 在数据库层不可编辑。更正只能形成新的追加事实（例如冲正）。
CREATE TRIGGER auction_settlements_append_only
BEFORE UPDATE OR DELETE ON public.auction_settlements
FOR EACH ROW EXECUTE FUNCTION public.reject_compensation_fact_mutation();

CREATE TRIGGER lot_settlements_append_only
BEFORE UPDATE OR DELETE ON public.lot_settlements
FOR EACH ROW EXECUTE FUNCTION public.reject_compensation_fact_mutation();

CREATE TRIGGER compensation_ledger_append_only
BEFORE UPDATE OR DELETE ON public.compensation_ledger
FOR EACH ROW EXECUTE FUNCTION public.reject_compensation_fact_mutation();

CREATE TRIGGER compensation_pool_reversals_append_only
BEFORE UPDATE OR DELETE ON public.compensation_pool_reversals
FOR EACH ROW EXECUTE FUNCTION public.reject_compensation_fact_mutation();
