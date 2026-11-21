-- 补强代理授权历史、realtime 来源与终态拍卖/lot 的数据库不变量。
-- owner: auction-storage
--
-- 不变量：
-- 1. 代理当前投影与连续转换历史一致；realtime 竞价来源引用真实命令，历史恢复重挂来源经命令账本或 Worker 审计证明。
-- 2. 已结算或撤销的拍卖不保留活动 lot；迁移前矛盾行失败关闭。
-- 3. 依赖 000079 的已命名 CHECK 基线缺失时明确拒绝升级。
-- Breaking：新增跨行约束并收紧事件来源；升级前须停止旧写入角色。

LOCK TABLE
    public.audit_records,
    public.auction_commands,
    public.auctions,
    public.bid_proxy_authorization_transitions,
    public.bid_proxy_authorizations,
    public.inbound_mail,
    public.lots,
    public.realtime_events,
    public.settlement_recoveries,
    public.settlement_recovery_commands,
    public.user_mail_commands
IN SHARE ROW EXCLUSIVE MODE;

-- 迁移 000079 只能依赖这四项已命名的强约束；未知历史约束无法从当前行反推，缺少基线时失败关闭。
DO $block$
DECLARE
    missing text;
BEGIN
    SELECT string_agg(required.name, ', ' ORDER BY required.name)
    INTO missing
    FROM (VALUES
        ('user_mail_commands_command_kind_check'),
        ('user_mail_commands_result_code_check'),
        ('user_mail_commands_user_id_check'),
        ('inbound_mail_mail_kind_check')
    ) AS required(name)
    WHERE NOT EXISTS (
        SELECT 1
        FROM pg_catalog.pg_constraint AS constraint_row
        WHERE constraint_row.conrelid = CASE
                WHEN required.name LIKE 'user_mail_commands_%'
                    THEN 'public.user_mail_commands'::regclass
                ELSE 'public.inbound_mail'::regclass
            END
          AND constraint_row.conname = required.name
          AND constraint_row.contype = 'c'
          AND constraint_row.convalidated
    );
    IF missing IS NOT NULL THEN
        RAISE EXCEPTION '000079 预期 CHECK 基线缺失或未验证：%' , missing
            USING ERRCODE = '23514';
    END IF;
END;
$block$;

-- 000034 的 CHECK 曾允许比较结果为 NULL 的 lowered 行；把整个矩阵强制为 TRUE。
ALTER TABLE public.bid_proxy_authorization_transitions
    DROP CONSTRAINT bid_proxy_authorization_transitions_matrix_check,
    ADD CONSTRAINT bid_proxy_authorization_transitions_matrix_check CHECK ((
        (
            transition_kind = 'created'
            AND transition_number = 1
            AND old_maximum_amount IS NULL
            AND old_mode IS NULL
            AND old_status IS NULL
            AND new_status = 'active'
            AND reason IS NULL
            AND resulting_state_version = 1
        )
        OR (
            transition_kind = 'lowered'
            AND old_maximum_amount IS NOT NULL
            AND new_maximum_amount < old_maximum_amount
            AND old_mode = new_mode
            AND old_status IN ('active', 'final_fast')
            AND old_status = new_status
            AND (old_status = 'active' OR old_mode = 'fast')
            AND reason IS NULL
        )
        OR (
            transition_kind = 'final_fast'
            AND old_maximum_amount = new_maximum_amount
            AND old_mode = 'slow'
            AND new_mode = 'fast'
            AND old_status = 'active'
            AND new_status = 'final_fast'
            AND reason IS NULL
        )
        OR (
            transition_kind = 'exhausted'
            AND old_maximum_amount = new_maximum_amount
            AND old_mode = new_mode
            AND old_status IN ('active', 'final_fast')
            AND new_status = 'exhausted'
            AND reason IN ('strictly_defeated', 'ranked_below')
        )
        OR (
            transition_kind = 'frozen'
            AND old_maximum_amount = new_maximum_amount
            AND old_mode = new_mode
            AND old_status IN ('active', 'final_fast')
            AND new_status = 'frozen'
            AND reason = 'eligibility_lost'
        )
        OR (
            transition_kind = 'closed'
            AND old_maximum_amount = new_maximum_amount
            AND old_mode = new_mode
            AND old_status IN ('active', 'final_fast')
            AND new_status = 'closed'
            AND reason = 'competition_closed'
        )
    ) IS TRUE);





-- 代理转换必须形成连续状态链，且当前授权投影必须等于最后一条转换。
DO $block$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM (
            SELECT transition.*,
                   row_number() OVER (
                       PARTITION BY authorization_id ORDER BY transition_number
                   ) AS expected_number,
                   lag(new_maximum_amount) OVER (
                       PARTITION BY authorization_id ORDER BY transition_number
                   ) AS previous_maximum,
                   lag(new_mode) OVER (
                       PARTITION BY authorization_id ORDER BY transition_number
                   ) AS previous_mode,
                   lag(new_status) OVER (
                       PARTITION BY authorization_id ORDER BY transition_number
                   ) AS previous_status,
                   lag(reason) OVER (
                       PARTITION BY authorization_id ORDER BY transition_number
                   ) AS previous_reason,
                   lag(resulting_state_version) OVER (
                       PARTITION BY authorization_id ORDER BY transition_number
                   ) AS previous_version
            FROM public.bid_proxy_authorization_transitions AS transition
        ) AS history
        WHERE history.transition_number <> history.expected_number
           OR history.resulting_state_version <> history.transition_number
           OR (history.previous_version IS NOT NULL AND (
               history.old_maximum_amount IS DISTINCT FROM history.previous_maximum
               OR history.old_mode IS DISTINCT FROM history.previous_mode
               OR history.old_status IS DISTINCT FROM history.previous_status
               OR history.resulting_state_version <> history.previous_version + 1
           ))
    ) OR EXISTS (
        SELECT 1
        FROM public.bid_proxy_authorizations AS proxy_authorization
        LEFT JOIN LATERAL (
            SELECT transition.*
            FROM public.bid_proxy_authorization_transitions AS transition
            WHERE transition.authorization_id = proxy_authorization.authorization_id
            ORDER BY transition.transition_number DESC
            LIMIT 1
        ) AS latest ON true
        WHERE latest.authorization_id IS NULL
           OR proxy_authorization.state_version IS DISTINCT FROM latest.resulting_state_version
           OR proxy_authorization.maximum_amount IS DISTINCT FROM latest.new_maximum_amount
           OR proxy_authorization.mode IS DISTINCT FROM latest.new_mode
           OR proxy_authorization.status IS DISTINCT FROM latest.new_status
           OR proxy_authorization.terminal_reason IS DISTINCT FROM
              CASE WHEN latest.new_status IN ('active', 'final_fast') THEN NULL
                   ELSE latest.reason END
    ) THEN
        RAISE EXCEPTION '代理授权转换历史与当前投影不一致；迁移前须人工核对'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1 FROM public.realtime_events AS event
        WHERE (event.origin_bid_id IS NULL) <> (event.origin_fact_index IS NULL)
           OR (event.origin_bid_id IS NOT NULL AND event.kind <> 'lot.bid_accepted')
           OR (event.kind = 'lot.bid_accepted'
               AND (event.origin_command_id IS NULL OR event.origin_bid_id IS NULL
                    OR event.origin_fact_index IS NULL OR event.state_version IS NULL))
           OR (
               event.origin_command_id IS NOT NULL
               AND NOT EXISTS (
                   SELECT 1 FROM public.auction_commands AS command
                   WHERE command.command_id = event.origin_command_id
               )
               AND NOT EXISTS (
                   SELECT 1
                   FROM public.settlement_recoveries AS recovery
                   WHERE recovery.stage IN ('completed', 'refunding')
                     AND recovery.resolution IS NOT NULL
                     AND recovery.relisted_auction_id IS NOT NULL
                     AND recovery.relisted_lot_id IS NOT NULL
                     AND event.kind = 'preview_catalog_changed'
                     AND event.schema_version = 1
                     AND event.state_version IS NOT NULL
                     AND event.stream_key = 'auction:' || recovery.relisted_auction_id::text
                     AND jsonb_typeof(event.public_payload) = 'object'
                     AND jsonb_typeof(event.public_payload->'preview_revision') = 'number'
                     AND event.public_payload->>'preview_revision' ~ '^[1-9][0-9]*$'
                     AND event.public_payload = jsonb_build_object(
                         'world', recovery.world_id,
                         'auction_id', recovery.relisted_auction_id,
                         'preview_revision', event.public_payload->'preview_revision'
                     )
                     AND (
                         EXISTS (
                             SELECT 1
                             FROM public.settlement_recovery_commands AS command
                             WHERE command.command_id = event.origin_command_id
                               AND command.recovery_id = recovery.id
                               AND command.action = recovery.resolution
                               AND command.result_code = 'ok'
                               AND command.result_payload->>'recovery_id' = recovery.id::text
                               AND command.result_payload->>'relisted_auction_id' =
                                   recovery.relisted_auction_id::text
                               AND command.result_payload->>'relisted_lot_id' =
                                   recovery.relisted_lot_id::text
                         )
                         OR EXISTS (
                             SELECT 1
                             FROM public.audit_records AS audit
                             WHERE audit.action = 'auction.recovery.relisted'
                               AND audit.entity_type = 'settlement_recovery'
                               AND audit.entity_id = recovery.id::text
                               AND audit.result = 'success'
                               AND audit.source = 'worker'
                               AND audit.metadata->>'origin_command_id' =
                                   event.origin_command_id::text
                               AND audit.metadata->>'resolution' = recovery.resolution
                               AND audit.metadata->>'relisted_auction_id' =
                                   recovery.relisted_auction_id::text
                               AND audit.metadata->>'relisted_lot_id' =
                                   recovery.relisted_lot_id::text
                         )
                         OR EXISTS (
                             SELECT 1
                             FROM public.audit_records AS audit
                             WHERE audit.action = 'auction.recovery.' || recovery.resolution
                               AND audit.entity_type = 'auction'
                               AND audit.entity_id = recovery.id::text
                               AND audit.result = 'success'
                               AND audit.source = 'admin'
                               AND audit.metadata->>'command_id' = event.origin_command_id::text
                               AND audit.metadata->>'relisted_auction_id' =
                                   recovery.relisted_auction_id::text
                               AND audit.metadata->>'relisted_lot_id' =
                                   recovery.relisted_lot_id::text
                         )
                     )
               )
           )
    ) THEN
        RAISE EXCEPTION 'realtime 事件来源不完整或引用不存在的命令；迁移前须人工核对'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM public.auctions AS auction
        JOIN public.lots AS lot ON lot.auction_id = auction.id
        WHERE (auction.status = 'settled' AND lot.status IN ('pending', 'live'))
           OR (auction.status IN ('settled', 'cancelled') AND lot.status = 'live')
    ) THEN
        RAISE EXCEPTION '终态拍卖仍包含未完成 lot；迁移前须人工核对'
            USING ERRCODE = '23514';
    END IF;
END;
$block$;

ALTER TABLE public.realtime_events
    ADD CONSTRAINT realtime_events_bid_origin_shape_000118 CHECK (
        (origin_bid_id IS NULL) = (origin_fact_index IS NULL)
        AND (origin_bid_id IS NULL OR kind = 'lot.bid_accepted')
    ) NOT VALID,
    ADD CONSTRAINT realtime_events_terminal_lot_shape_000118 CHECK (
        kind <> 'lot.bid_accepted'
        OR (origin_command_id IS NOT NULL AND origin_bid_id IS NOT NULL
            AND origin_fact_index IS NOT NULL AND state_version IS NOT NULL)
    ) NOT VALID,
    ADD CONSTRAINT realtime_events_origin_command_fkey_000118
        FOREIGN KEY (origin_command_id) REFERENCES public.auction_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED NOT VALID;

ALTER TABLE public.realtime_events
    VALIDATE CONSTRAINT realtime_events_bid_origin_shape_000118,
    VALIDATE CONSTRAINT realtime_events_terminal_lot_shape_000118;

COMMENT ON CONSTRAINT realtime_events_origin_command_fkey_000118
ON public.realtime_events IS
    '迁移前置校验已证明历史恢复重挂来源；NOT VALID 保留只追加的旧事件，同时约束新写入。';

CREATE FUNCTION public.validate_bid_proxy_authorization_history_000118()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
DECLARE
    checked_authorization uuid;
    current_row record;
    latest_row record;
BEGIN
    checked_authorization := NEW.authorization_id;
    SELECT proxy_authorization.* INTO current_row
    FROM public.bid_proxy_authorizations AS proxy_authorization
    WHERE proxy_authorization.authorization_id = checked_authorization;
    IF NOT FOUND THEN
        RETURN NULL;
    END IF;

    IF EXISTS (
        SELECT 1
        FROM (
            SELECT transition.*,
                   row_number() OVER (
                       PARTITION BY authorization_id ORDER BY transition_number
                   ) AS expected_number,
                   lag(new_maximum_amount) OVER (
                       PARTITION BY authorization_id ORDER BY transition_number
                   ) AS previous_maximum,
                   lag(new_mode) OVER (
                       PARTITION BY authorization_id ORDER BY transition_number
                   ) AS previous_mode,
                   lag(new_status) OVER (
                       PARTITION BY authorization_id ORDER BY transition_number
                   ) AS previous_status,
                   lag(resulting_state_version) OVER (
                       PARTITION BY authorization_id ORDER BY transition_number
                   ) AS previous_version
            FROM public.bid_proxy_authorization_transitions AS transition
            WHERE transition.authorization_id = checked_authorization
        ) AS history
        WHERE history.transition_number <> history.expected_number
           OR history.resulting_state_version <> history.transition_number
           OR (history.previous_version IS NOT NULL AND (
               history.old_maximum_amount IS DISTINCT FROM history.previous_maximum
               OR history.old_mode IS DISTINCT FROM history.previous_mode
               OR history.old_status IS DISTINCT FROM history.previous_status
               OR history.resulting_state_version <> history.previous_version + 1
           ))
    ) THEN
        RAISE EXCEPTION '代理授权转换历史不是连续状态链'
            USING ERRCODE = '23514';
    END IF;

    SELECT transition.* INTO latest_row
    FROM public.bid_proxy_authorization_transitions AS transition
    WHERE transition.authorization_id = checked_authorization
    ORDER BY transition.transition_number DESC
    LIMIT 1;
    IF NOT FOUND
       OR current_row.state_version IS DISTINCT FROM latest_row.resulting_state_version
       OR current_row.maximum_amount IS DISTINCT FROM latest_row.new_maximum_amount
       OR current_row.mode IS DISTINCT FROM latest_row.new_mode
       OR current_row.status IS DISTINCT FROM latest_row.new_status
       OR current_row.terminal_reason IS DISTINCT FROM (
           CASE WHEN latest_row.new_status IN ('active', 'final_fast') THEN NULL
                ELSE latest_row.reason END
       ) THEN
        RAISE EXCEPTION '代理授权当前投影不等于最新转换'
            USING ERRCODE = '23514';
    END IF;
    RETURN NULL;
END;
$function$;

CREATE CONSTRAINT TRIGGER bid_proxy_authorization_history_matches_state_000118
AFTER INSERT OR UPDATE ON public.bid_proxy_authorizations
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.validate_bid_proxy_authorization_history_000118();
CREATE CONSTRAINT TRIGGER bid_proxy_transition_history_matches_state_000118
AFTER INSERT ON public.bid_proxy_authorization_transitions
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.validate_bid_proxy_authorization_history_000118();

-- 终态拍卖与 lot 状态由同一提交快照校验；事务 advisory lock 串行化跨行写入。
CREATE FUNCTION public.validate_auction_terminal_lots_000118()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
DECLARE
    candidate_ids uuid[];
    checked_id uuid;
BEGIN
    IF TG_TABLE_NAME = 'auctions' THEN
        IF TG_OP = 'DELETE' THEN
            candidate_ids := ARRAY[OLD.id];
        ELSE
            candidate_ids := ARRAY[NEW.id];
        END IF;
    ELSIF TG_OP = 'INSERT' THEN
        candidate_ids := ARRAY[NEW.auction_id];
    ELSIF TG_OP = 'DELETE' THEN
        candidate_ids := ARRAY[OLD.auction_id];
    ELSE
        candidate_ids := ARRAY[OLD.auction_id, NEW.auction_id];
    END IF;

    FOR checked_id IN
        SELECT DISTINCT candidate_id
        FROM unnest(candidate_ids) AS candidate(candidate_id)
        WHERE candidate_id IS NOT NULL
        ORDER BY candidate_id
    LOOP
        PERFORM pg_catalog.pg_advisory_xact_lock(
            pg_catalog.hashtextextended('auction-terminal-lots:' || checked_id::text, 0)
        );
        IF EXISTS (
            SELECT 1
            FROM public.auctions AS auction
            JOIN public.lots AS lot ON lot.auction_id = auction.id
            WHERE auction.id = checked_id
              AND ((auction.status = 'settled' AND lot.status IN ('pending', 'live'))
                   OR (auction.status IN ('settled', 'cancelled') AND lot.status = 'live'))
        ) THEN
            RAISE EXCEPTION '终态拍卖仍包含活动或未完成 lot：%', checked_id
                USING ERRCODE = '23514';
        END IF;
    END LOOP;
    RETURN NULL;
END;
$function$;

CREATE CONSTRAINT TRIGGER auction_terminal_lots_consistent_000118
AFTER INSERT OR UPDATE OR DELETE ON public.auctions
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.validate_auction_terminal_lots_000118();
CREATE CONSTRAINT TRIGGER lot_terminal_auction_consistent_000118
AFTER INSERT OR UPDATE OR DELETE ON public.lots
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.validate_auction_terminal_lots_000118();







INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (118, 'breaking');
