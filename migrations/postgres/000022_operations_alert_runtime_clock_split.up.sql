-- 配置状态发布时间与成功扫描时间来自两条独立时钟路径，二者不存在先后约束。
-- 状态发布仍由存储适配器单调校验；扫描事务只单调推进 last_observed_at。
ALTER TABLE public.operations_alert_runtime_state
    DROP CONSTRAINT operations_alert_runtime_state_time_check;

COMMENT ON COLUMN public.operations_alert_runtime_state.updated_at IS
    'Worker 最近一次配置状态发布时间，不表示扫描时间';

COMMENT ON COLUMN public.operations_alert_runtime_state.last_observed_at IS
    'Operations alert 最近一次成功扫描水位';
