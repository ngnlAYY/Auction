-- Worker 租约和失败状态只追加到既有轮次表；due 与租约仍由数据库条件更新决定。
ALTER TABLE public.scheduled_rounds
    ADD COLUMN lease_owner text
        CHECK (lease_owner IS NULL OR (lease_owner = btrim(lease_owner) AND lease_owner <> '')),
    ADD COLUMN lease_until timestamptz,
    ADD COLUMN attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    ADD COLUMN last_failure_code text,
    ADD COLUMN last_failure_detail text;

CREATE INDEX scheduled_rounds_worker_due_idx
    ON public.scheduled_rounds (lease_until, preview_due_at, open_due_at, id)
    WHERE status = 'active' AND open_completed_at IS NULL;
