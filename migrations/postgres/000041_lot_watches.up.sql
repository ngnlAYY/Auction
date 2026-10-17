-- 装备标记、连续领先周期与独立提醒来源。迁移只补标记，不生成过去提醒。
CREATE TABLE public.lot_watches (
    user_id uuid NOT NULL REFERENCES public.users(id),
    lot_id uuid NOT NULL REFERENCES public.lots(id),
    watched boolean NOT NULL,
    channel text NOT NULL DEFAULT 'mm' CHECK (channel = 'mm'),
    version bigint NOT NULL CHECK (version > 0),
    updated_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (user_id, lot_id)
);
CREATE INDEX lot_watches_lot_idx ON public.lot_watches (lot_id, user_id);

-- 周期是首次取得领先的状态版本，不随同人加价变化。
CREATE TABLE public.lot_watch_leadership (
    lot_id uuid PRIMARY KEY REFERENCES public.lots(id),
    leader_user_id uuid REFERENCES public.users(id),
    cycle bigint NOT NULL CHECK (cycle >= 0)
);
INSERT INTO public.lot_watch_leadership (lot_id, leader_user_id, cycle)
SELECT id, leader_user_id, state_version FROM public.lots;

INSERT INTO public.lot_watches (user_id, lot_id, watched, channel, version)
SELECT DISTINCT user_id, lot_id, true, 'mm', 1
FROM public.bid_proxy_authorizations WHERE status IN ('active', 'final_fast');

ALTER TABLE public.deliveries DROP CONSTRAINT deliveries_origin_kind_check;
ALTER TABLE public.deliveries ADD CONSTRAINT deliveries_origin_kind_check CHECK (
    origin_kind IN ('settlement', 'settlement_recovery_action', 'user_key', 'manual',
                    'mail_intake', 'operations_alert', 'lot_watch')
);

-- 只有 watch owner 能生成独立来源；发送身份绑定完整冻结 payload 与收件人。
CREATE TABLE public.lot_watch_notifications (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL,
    lot_id uuid NOT NULL,
    leadership_cycle bigint NOT NULL CHECK (leadership_cycle >= 0),
    displaced_bid_id uuid NOT NULL REFERENCES public.bids(id),
    watch_version bigint NOT NULL CHECK (watch_version > 0),
    delivery_id uuid NOT NULL UNIQUE REFERENCES public.deliveries(id),
    command_id uuid NOT NULL UNIQUE,
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    recipient text NOT NULL,
    payload jsonb NOT NULL CHECK (jsonb_typeof(payload) = 'object'),
    created_at timestamptz NOT NULL,
    invalidated_at timestamptz,
    invalidation_reason text CHECK (invalidation_reason IN ('unwatched', 'leading_again')),
    FOREIGN KEY (user_id, lot_id) REFERENCES public.lot_watches(user_id, lot_id),
    UNIQUE (lot_id, user_id, leadership_cycle),
    CHECK ((invalidated_at IS NULL) = (invalidation_reason IS NULL))
);
CREATE INDEX lot_watch_notifications_live_idx
ON public.lot_watch_notifications (lot_id, user_id) WHERE invalidated_at IS NULL;

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
SELECT gen_random_uuid(), 'system', 'migration-000041', 'lot_watch.backfill', 'lot',
       lot_id::text, 'success', 'system',
       jsonb_build_object('user_id', user_id, 'watched', true, 'version', version)
FROM public.lot_watches;
