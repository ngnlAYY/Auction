-- 旧账户保留现金保护；仅显式初始化的新账户可以进入只记录模式。
ALTER TABLE public.settlement_funding_accounts
    ADD COLUMN accounting_mode text NOT NULL DEFAULT 'strict_cash'
        CHECK (accounting_mode IN ('strict_cash','record_only')),
    ADD COLUMN policy_version bigint NOT NULL DEFAULT 1 CHECK (policy_version > 0);
ALTER TABLE public.settlement_funding_accounts ADD CONSTRAINT settlement_funding_record_only_not_cash_frozen
    CHECK (accounting_mode <> 'record_only' OR status = 'ready');
CREATE UNIQUE INDEX settlement_funding_single_record_only
    ON public.settlement_funding_accounts ((true)) WHERE accounting_mode='record_only';

-- record_*是冻结业务金额的有限会计来源，不代表读取或授权真实账号现金。
ALTER TABLE public.settlement_funding_sources DROP CONSTRAINT settlement_funding_sources_kind_check;
ALTER TABLE public.settlement_funding_sources ADD CONSTRAINT settlement_funding_sources_kind_check
    CHECK (kind IN ('reconciliation','collected_payment','returned_delivery',
                   'record_opening','record_internal_purchase','record_compensation','record_carrier'));

CREATE FUNCTION public.reject_funding_policy_mutation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.accounting_mode IS DISTINCT FROM OLD.accounting_mode
       OR NEW.policy_version IS DISTINCT FROM OLD.policy_version THEN
        RAISE EXCEPTION 'funding policy requires a dedicated migration' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER settlement_funding_policy_immutable
BEFORE UPDATE OF accounting_mode,policy_version ON public.settlement_funding_accounts
FOR EACH ROW EXECUTE FUNCTION public.reject_funding_policy_mutation();

-- 只记录账户的现金健康项不适用；保留债务/任务backlog，不伪造余额或低余额告警。
ALTER TABLE public.settlement_recovery_health DROP CONSTRAINT settlement_recovery_health_balance_status_check;
ALTER TABLE public.settlement_recovery_health ADD CONSTRAINT settlement_recovery_health_balance_status_check
    CHECK (balance_status IN ('unknown','ok','low','stale','busy','unavailable','not_applicable'));
ALTER TABLE public.settlement_recovery_health DROP CONSTRAINT settlement_recovery_health_evidence_status_check;
ALTER TABLE public.settlement_recovery_health ADD CONSTRAINT settlement_recovery_health_evidence_status_check
    CHECK (evidence_status IN ('unknown','fresh','stale','unavailable','not_applicable'));
