-- Operations alert 的启用状态属于 Worker 配置，但 Admin 只能读取共享的非敏感健康投影。
-- 单例状态同时保存成功扫描水位，避免“功能关闭”或“扫描结果为空”被误报为运行异常。
CREATE TABLE public.operations_alert_runtime_state (
    singleton_id SMALLINT PRIMARY KEY,
    enabled BOOLEAN NOT NULL,
    last_observed_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT operations_alert_runtime_state_singleton_check CHECK (singleton_id = 1),
    CONSTRAINT operations_alert_runtime_state_disabled_check CHECK (
        enabled OR last_observed_at IS NULL
    ),
    CONSTRAINT operations_alert_runtime_state_time_check CHECK (
        last_observed_at IS NULL OR last_observed_at <= updated_at
    )
);

INSERT INTO public.operations_alert_runtime_state
    (singleton_id, enabled, last_observed_at, updated_at)
VALUES (1, false, NULL, CURRENT_TIMESTAMP);
