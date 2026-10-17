-- 账号级警告渠道。部署须先停止旧 Worker，避免旧进程忽略新的 PM 来源。
ALTER TABLE public.users
    ADD COLUMN lot_watch_channel text NOT NULL DEFAULT 'mm' CHECK (lot_watch_channel IN ('mm', 'pm')),
    ADD COLUMN lot_watch_channel_version bigint NOT NULL DEFAULT 1 CHECK (lot_watch_channel_version > 0);

-- 历史 MM 的 payload/digest/command_id 不改写，旧偏好版本保持 NULL。
ALTER TABLE public.lot_watch_notifications
    ADD COLUMN channel text NOT NULL DEFAULT 'mm' CHECK (channel IN ('mm', 'pm')),
    ADD COLUMN preference_version bigint CHECK (preference_version > 0),
    ALTER COLUMN delivery_id DROP NOT NULL,
    ALTER COLUMN recipient DROP NOT NULL,
    ADD UNIQUE (id, channel),
    ADD CHECK ((channel = 'mm' AND delivery_id IS NOT NULL AND recipient IS NOT NULL)
        OR (channel = 'pm' AND delivery_id IS NULL AND preference_version IS NOT NULL
            AND (recipient IS NULL OR recipient ~ '^[1-9][0-9]{0,31}$')));

CREATE TABLE public.lot_watch_pm_deliveries (
    notification_id uuid PRIMARY KEY,
    channel text NOT NULL DEFAULT 'pm' CHECK (channel = 'pm'),
    status text NOT NULL CHECK (status IN ('pending','claimed','sending','succeeded','suppressed','manual_reconcile')),
    fencing_version bigint NOT NULL DEFAULT 1 CHECK (fencing_version > 0),
    attempt_count bigint NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    reconcile_attempt_count bigint NOT NULL DEFAULT 0 CHECK (reconcile_attempt_count >= 0),
    lease_token uuid,
    lease_expires_at timestamptz,
    lease_kind text CHECK (lease_kind IN ('send','reconcile')),
    worker_id text,
    claimed_at timestamptz,
    sending_at timestamptz,
    sender_forum_account_id text CHECK (sender_forum_account_id ~ '^[1-9][0-9]{0,31}$'),
    external_reference text CHECK (external_reference ~ '^[1-9][0-9]{0,31}$'),
    failure_code text CHECK (failure_code IN ('recipient_missing','source_invalid','prepare_unavailable','outcome_unknown','evidence_missing','sender_unavailable')),
    reconcile_requested_at timestamptz,
    completed_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY (notification_id, channel) REFERENCES public.lot_watch_notifications(id, channel),
    CHECK ((lease_token IS NULL AND lease_expires_at IS NULL AND lease_kind IS NULL AND worker_id IS NULL)
        OR (lease_token IS NOT NULL AND lease_expires_at IS NOT NULL AND lease_kind IS NOT NULL AND worker_id IS NOT NULL)),
    CHECK ((sending_at IS NULL) = (sender_forum_account_id IS NULL)),
    CHECK (sending_at IS NULL OR status IN ('sending','succeeded','manual_reconcile')),
    CHECK (status <> 'sending' OR (sending_at IS NOT NULL AND lease_kind = 'send' AND lease_token IS NOT NULL)),
    CHECK (status <> 'claimed' OR (sending_at IS NULL AND lease_kind = 'send' AND lease_token IS NOT NULL)),
    CHECK (status <> 'manual_reconcile' OR sending_at IS NOT NULL),
    CHECK (status <> 'succeeded' OR (sending_at IS NOT NULL AND external_reference IS NOT NULL AND completed_at IS NOT NULL)),
    CHECK (failure_code IS DISTINCT FROM 'recipient_missing' OR (status = 'suppressed' AND sending_at IS NULL AND lease_token IS NULL))
);
-- 领取时间持久轮转；准备失败/租约过期不会让同一坏收件人饿死后续任务。
CREATE INDEX lot_watch_pm_due_idx ON public.lot_watch_pm_deliveries(COALESCE(claimed_at,created_at),created_at,notification_id) WHERE status IN ('pending','claimed');
CREATE INDEX lot_watch_pm_expiry_idx ON public.lot_watch_pm_deliveries(lease_expires_at) WHERE lease_token IS NOT NULL;
CREATE INDEX lot_watch_pm_reconcile_idx ON public.lot_watch_pm_deliveries(reconcile_requested_at) WHERE status = 'manual_reconcile';

-- 即使未来新增 writer，也不能解除已开始发送的不可重寄 fence 或更换 sender。
CREATE FUNCTION public.guard_lot_watch_pm_sending() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.sending_at IS NOT NULL AND
       (NEW.sending_at IS DISTINCT FROM OLD.sending_at OR
        NEW.sender_forum_account_id IS DISTINCT FROM OLD.sender_forum_account_id OR
        NEW.status NOT IN ('sending','succeeded','manual_reconcile')) THEN
        RAISE EXCEPTION 'PM sending fence is immutable' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER lot_watch_pm_sending_guard BEFORE UPDATE ON public.lot_watch_pm_deliveries
FOR EACH ROW EXECUTE FUNCTION public.guard_lot_watch_pm_sending();
