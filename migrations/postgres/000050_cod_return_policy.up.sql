-- COD 退回策略可配置：每个 recovery 冻结自己的退回上限，默认三次。
-- 第 1..limit-1 次退回重发 COD，第 limit 次退回装备给卖家并拉黑买家。
ALTER TABLE public.settlement_recoveries
    ADD COLUMN cod_return_limit integer NOT NULL DEFAULT 3
        CHECK (cod_return_limit BETWEEN 1 AND 10);

ALTER TABLE public.settlement_recoveries
    DROP CONSTRAINT settlement_recoveries_cod_return_count_check;

ALTER TABLE public.settlement_recoveries
    ADD CONSTRAINT settlement_recoveries_cod_return_count_check CHECK (
        cod_return_count BETWEEN 0 AND cod_return_limit
    );

-- buyer_blacklist action 保存创建时冻结的公开理由；旧行保持 NULL。
ALTER TABLE public.settlement_recovery_actions
    ADD COLUMN reason text;

ALTER TABLE public.settlement_recovery_actions
    ADD CONSTRAINT settlement_recovery_actions_reason_check CHECK (
        reason IS NULL
        OR (kind = 'buyer_blacklist' AND length(reason) BETWEEN 1 AND 256)
    );
