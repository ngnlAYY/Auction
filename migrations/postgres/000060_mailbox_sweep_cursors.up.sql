-- 维护游标只记录一次只读核对机会，不是投递、资金、领取或草稿丢弃的成功证明。
-- 仅在逐条开始核对前推进；时间或条数预算未处理的批次后缀不推进，到尾部后稳定回绕。
CREATE TABLE public.mailbox_sweep_cursors (
    world_id text PRIMARY KEY CHECK (world_id IN ('main', 'isekai')),
    last_created_at timestamptz NOT NULL CHECK (isfinite(last_created_at)),
    last_delivery_id uuid NOT NULL CHECK (last_delivery_id <> '00000000-0000-0000-0000-000000000000')
);

-- 游标保留排序键而不引用仍需存在的 delivery，已清理的历史行不会阻断后续轮转。
CREATE INDEX deliveries_mailbox_sweep_order
    ON public.deliveries ((COALESCE(payload->>'world', 'main')), created_at, id)
    WHERE status = 'manual_reconcile'
      AND payload->>'operation' IN ('send', 'take')
      AND external_reference IS NULL
      AND sending_at IS NOT NULL
      AND completed_at IS NOT NULL
      AND origin_kind IN ('settlement', 'mail_intake');
