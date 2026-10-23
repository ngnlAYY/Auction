-- Owner：application/delivery 的分组节奏；SQL 与旧载荷映射由 storage/delivery_scheduler 独占。
-- 只建立节奏投影，不改变任何投递负载、命令、结果或人工核对记录。
CREATE FUNCTION public.delivery_send_category(origin text, payload jsonb)
RETURNS text LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE
        WHEN payload ->> 'operation' IS DISTINCT FROM 'send' THEN NULL
        WHEN origin = 'lot_watch' THEN 'outbid'
        WHEN origin = 'operations_alert' THEN 'operations'
        WHEN origin = 'mail_intake' AND payload ->> 'template'
            IN ('register_key', 'register_result', 'rebind_result', 'user_assets_rejected')
            THEN 'account'
        WHEN jsonb_typeof(payload -> 'equipment_secret_refs') = 'array'
             AND payload -> 'equipment_secret_refs' <> '[]'::jsonb THEN 'equipment'
        WHEN jsonb_typeof(payload -> 'attachment_ids') = 'array'
             AND payload -> 'attachment_ids' <> '[]'::jsonb THEN 'equipment'
        WHEN origin IN ('settlement', 'settlement_recovery_action') THEN 'settlement'
        WHEN origin = 'mail_intake' THEN 'intake'
        ELSE 'manual'
    END
$$;

CREATE TABLE public.delivery_send_cadence (
    category text PRIMARY KEY CHECK (category IN
        ('outbid', 'account', 'intake', 'settlement', 'equipment', 'operations', 'manual')),
    last_started_at timestamptz CHECK (last_started_at IS NULL OR isfinite(last_started_at))
);

COMMENT ON TABLE public.delivery_send_cadence IS
    '单邮件账号跨世界的七类 Send 起点；sending fence 同事务更新，未知/失败不退还间隔';

-- 仅从已有发送起点初始化高水位；未发送的 claimed/prepared 不消耗间隔，不回填历史结果。
INSERT INTO public.delivery_send_cadence (category, last_started_at)
SELECT categories.category, MAX(deliveries.sending_at)
FROM unnest(ARRAY['outbid', 'account', 'intake', 'settlement', 'equipment', 'operations', 'manual'])
    AS categories(category)
LEFT JOIN public.deliveries AS deliveries
    ON public.delivery_send_category(deliveries.origin_kind, deliveries.payload) = categories.category
GROUP BY categories.category;

CREATE INDEX deliveries_priority_due_idx ON public.deliveries
    ((CASE WHEN origin_kind = 'lot_watch' THEN 0 ELSE 1 END), created_at, id)
    WHERE status IN ('pending', 'claimed');

-- NOTIFY 只在外层事务提交后送达；payload 为空，不泄漏邮件、用户或凭据。
-- 不对 claimed -> pending 的准备失败释放发通知，避免故障忙循环。
CREATE FUNCTION public.notify_delivery_ready() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('auction_delivery_ready', '');
    RETURN NEW;
END;
$$;

CREATE TRIGGER delivery_ready_after_insert
AFTER INSERT ON public.deliveries
FOR EACH ROW WHEN (NEW.origin_kind = 'lot_watch' AND NEW.status = 'pending')
EXECUTE FUNCTION public.notify_delivery_ready();
