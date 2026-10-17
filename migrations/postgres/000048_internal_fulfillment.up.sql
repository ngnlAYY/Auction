-- 内部满足的命令与完整非敏感证据独立于外部 Sent；不为旧记录回填清白或 UID。
CREATE TABLE public.settlement_internal_commands (
    command_id uuid PRIMARY KEY,
    obligation_id uuid NOT NULL REFERENCES public.settlement_obligations(id) ON DELETE RESTRICT,
    action text NOT NULL CHECK (action IN ('internal', 'collected', 'external_refund')),
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    uid text NOT NULL CHECK (uid ~ '^[1-9][0-9]*$'),
    evidence_digest bytea NOT NULL CHECK (octet_length(evidence_digest) = 32),
    evidence jsonb NOT NULL CHECK (jsonb_typeof(evidence) = 'object'),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (obligation_id, action)
);
COMMENT ON TABLE public.settlement_internal_commands IS
    '内部义务/真实实收资金化稳定命令及非敏感证明；成功事实只追加，不包含Cookie、token或页面';
