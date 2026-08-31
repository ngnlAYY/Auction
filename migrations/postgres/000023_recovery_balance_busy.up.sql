-- 角色处于战斗时 Item Shop 暂不可读；保留该可恢复状态，避免误报为协议漂移或凭据失效。
ALTER TABLE public.settlement_recovery_health
    DROP CONSTRAINT settlement_recovery_health_balance_status_check;

ALTER TABLE public.settlement_recovery_health
    ADD CONSTRAINT settlement_recovery_health_balance_status_check
    CHECK (balance_status IN ('unknown', 'ok', 'low', 'stale', 'busy', 'unavailable'));
