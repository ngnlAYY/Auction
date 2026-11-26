-- 寄售：未登录登记人以自填 HV 角色名登记非寄售挂单。
-- owner: auction-storage（consignment_listings 卖家身份、命令账本 actor 闭集与登记事件门）
-- 不变量 1、2、3、6、7、8：
--   - 挂单卖家二选一：本站账户 seller_user_id，或自填游戏名 seller_game_name；后者只能是非寄售（direct）。
--   - 匿名登记的命令账本 actor_kind = 'anonymous'，只允许 register，actor_ref 是原样自填名；
--     登记事件仍须同事务匹配命令（000132 起的事件门），名称必须与挂单逐字节一致。
--   - 同名（lower）同世界同装备至多一条未终结的匿名挂单；匿名挂单的自填名登记后不可改。
--   - 匿名挂单不能改价（改价门要求 actor_ref 等于账户 ID，NULL 失败关闭）；v2 撤回只接受与自填名
--     lower 等值的本站账户；过期、Admin 下架与强制终止沿用原有路径。
--   - 名称不含装备链接、key 或行分隔符（consignment_safe_observation_text），不让秘密经名称落库。
-- 等级：breaking。旧程序把 seller_user_id 解码为非空，读到匿名挂单会失败；须在停机门禁下与同版程序一起切换。

ALTER TABLE public.consignment_listings
    ALTER COLUMN seller_user_id DROP NOT NULL,
    ADD COLUMN seller_game_name text,
    ADD CONSTRAINT consignment_listing_seller CHECK ((
        CASE WHEN seller_user_id IS NULL
            THEN mode = 'direct' AND public.consignment_safe_observation_text(seller_game_name, 128)
            ELSE seller_game_name IS NULL
        END
    ) IS TRUE);

-- 匿名挂单的同名同装备占用（账户挂单由 consignment_listings_seller_active_idx 约束）。
CREATE UNIQUE INDEX consignment_listings_anonymous_active_idx
    ON public.consignment_listings (lower(seller_game_name), world_id, external_equipment_id)
    WHERE seller_user_id IS NULL AND state IN ('resolving', 'displayed', 'awaiting_deposit', 'available');
-- 按自填名计数（每小时登记数、未终结挂单数）与同名账户列出自己的匿名挂单。
CREATE INDEX consignment_listings_anonymous_seller_idx
    ON public.consignment_listings (lower(seller_game_name), created_at DESC, id DESC)
    WHERE seller_user_id IS NULL;
-- 全站匿名 resolving 总量上限的计数。
CREATE INDEX consignment_listings_anonymous_resolving_idx
    ON public.consignment_listings (created_at)
    WHERE seller_user_id IS NULL AND state = 'resolving';

-- 自填名与账户 ID 一样属于挂单的不可变身份；运行账号本就没有该列的 UPDATE 权限，这里对所有角色兜底。
CREATE FUNCTION public.consignment_guard_listing_seller_name() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NEW.seller_game_name IS DISTINCT FROM OLD.seller_game_name THEN
        RAISE EXCEPTION '寄售挂单的卖家游戏名不可修改'
            USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_seller';
    END IF;
    RETURN NEW;
END
$function$;
REVOKE ALL ON FUNCTION public.consignment_guard_listing_seller_name() FROM PUBLIC;
CREATE TRIGGER consignment_guard_listing_seller_name
    BEFORE UPDATE OF seller_game_name ON public.consignment_listings
    FOR EACH ROW EXECUTE FUNCTION public.consignment_guard_listing_seller_name();

-- 以下三段整段复制 000137 的最新文本，只加注明 000138 的分支。
ALTER TABLE public.consignment_commands
    DROP CONSTRAINT consignment_command_actor;
ALTER TABLE public.consignment_commands
    ADD CONSTRAINT consignment_command_actor CHECK ((
        CASE
            WHEN schema_version = 1 THEN CASE actor_kind
                WHEN 'public_user' THEN operation IN ('register', 'reprice', 'withdraw')
                    AND public.consignment_positive_uuid(actor_ref)
                -- 000138：未登录登记人只能登记非寄售，actor_ref 是原样自填游戏名。
                WHEN 'anonymous' THEN operation = 'register'
                    AND public.consignment_safe_observation_text(actor_ref, 128)
                WHEN 'admin' THEN operation = 'admin_delist'
                    AND public.consignment_valid_text(actor_ref, 256) AND strpos(actor_ref, ':') = 0
                WHEN 'worker' THEN operation = 'apply_lookup' AND public.consignment_valid_text(actor_ref, 256)
                WHEN 'system' THEN operation = 'auto_close' AND public.consignment_valid_text(actor_ref, 256)
            END
            WHEN schema_version = 2 THEN CASE actor_kind
                WHEN 'public_user' THEN operation IN (
                    'issue_reservation', 'withdraw', 'purchase', 'reprice', 'relist')
                    AND public.consignment_positive_uuid(actor_ref)
                WHEN 'admin' THEN operation IN (
                    'recheck_quarantine', 'release_quarantine', 'admin_delist', 'release_hold',
                    'confirm_position_loss', 'record_loss_case_outcome', 'recheck', 'resolve',
                    'record_position_loss_proof')
                    AND public.consignment_valid_text(actor_ref, 256) AND strpos(actor_ref, ':') = 0
                WHEN 'worker' THEN operation IN (
                    'expire_reservation', 'decide_envelope', 'append_quarantine_observation',
                    'start_disposition_write', 'apply_mail_proof', 'season_notice', 'season_close',
                    'authorize_dispatch', 'start_attempt', 'mark_write_started', 'record_evidence',
                    'acquire_return', 'start_return_take', 'consume_return',
                    'register_opening_attribution', 'certify_cod', 'prove_handoff', 'settle_cash',
                    'convert_refund', 'open_sanction',
                    'fulfill_withdraw_intent', 'resolve_return', 'auto_relist', 'remind',
                    'apply_sanction', 'deliver_right_notice')
                    AND public.consignment_valid_text(actor_ref, 256)
                WHEN 'system' THEN operation IN (
                    'release_quarantine', 'recheck_quarantine', 'acquire_return', 'reset_cycle')
                    AND public.consignment_valid_text(actor_ref, 256)
                -- 受限权利的浏览器与权利人（000137，J5）：actor_ref 分别是 64 位小写十六进制的浏览器绑定摘要
                -- 与 right_id 文本；二者没有登录账号，命令 ID 仍是 UUIDv4（见 consignment_command_uuid_version）。
                WHEN 'browser' THEN operation IN ('locate_right', 'challenge_right', 'verify_right')
                    AND actor_ref COLLATE "C" ~ '^[0-9a-f]{64}$'
                WHEN 'right_holder' THEN operation = 'claim_right'
                    AND public.consignment_positive_uuid(actor_ref)
            END
        END
    ) IS TRUE);;

ALTER TABLE public.consignment_commands
    DROP CONSTRAINT consignment_command_uuid_version;
ALTER TABLE public.consignment_commands
    ADD CONSTRAINT consignment_command_uuid_version CHECK ((
        pg_catalog.uuid_extract_version(command_id) =
        CASE actor_kind
            WHEN 'public_user' THEN 4
            WHEN 'admin' THEN 4
            WHEN 'worker' THEN 5
            WHEN 'system' THEN 5
            WHEN 'browser' THEN 4
            WHEN 'right_holder' THEN 4
            WHEN 'anonymous' THEN 4
        END
    ) IS TRUE);;

ALTER TABLE public.consignment_events
    DROP CONSTRAINT consignment_event_edge;
ALTER TABLE public.consignment_events
    ADD CONSTRAINT consignment_event_edge CHECK ((
        CASE
            WHEN state_version = 1 THEN event_kind = 'registered'
                AND actor_kind IN ('public_user', 'anonymous')
                AND from_state IS NULL AND to_state = 'resolving' AND reason_code IS NULL
            WHEN from_state IN ('resolving', 'displayed', 'awaiting_deposit') THEN
                CASE event_kind
                    WHEN 'repriced' THEN actor_kind = 'public_user' AND to_state = from_state
                        AND reason_code IS NULL
                    WHEN 'published' THEN actor_kind = 'worker' AND from_state = 'resolving'
                        AND to_state IN ('displayed', 'awaiting_deposit') AND reason_code IS NULL
                    WHEN 'rejected' THEN actor_kind = 'worker' AND from_state = 'resolving'
                        AND to_state = 'rejected'
                        AND public.consignment_reason_state(reason_code) IN ('rejected', 'rejected_with_snapshot')
                    WHEN 'expired' THEN actor_kind = 'system' AND to_state = 'expired'
                        AND reason_code = CASE from_state WHEN 'resolving' THEN 'lookup_deadline'
                            WHEN 'displayed' THEN 'display_expired' ELSE 'deposit_expired' END
                    WHEN 'closed' THEN to_state = 'closed' AND
                        CASE actor_kind
                            WHEN 'public_user' THEN reason_code = 'seller_cancelled'
                            WHEN 'admin' THEN reason_code = 'admin_delisted'
                            WHEN 'system' THEN reason_code IN (
                                'auction_lot_created', 'seller_ineligible', 'service_disabled')
                        END
                    WHEN 'deposit_accepted' THEN actor_kind IN ('worker', 'admin', 'system')
                        AND from_state = 'awaiting_deposit' AND to_state = 'deposit_accepted'
                        AND reason_code IS NULL
                    WHEN 'reservation_issued' THEN actor_kind = 'public_user'
                        AND from_state = 'awaiting_deposit' AND to_state = 'awaiting_deposit'
                        AND reason_code IS NULL
                    WHEN 'reservation_retired' THEN actor_kind = 'worker'
                        AND from_state = 'awaiting_deposit' AND to_state = 'awaiting_deposit'
                        AND reason_code IS NULL
                    WHEN 'season_closed' THEN actor_kind IN ('worker', 'system')
                        AND to_state = 'closed' AND reason_code = 'season_closed'
                END
            WHEN from_state = 'deposit_accepted' THEN
                CASE event_kind
                    WHEN 'available' THEN actor_kind = 'worker' AND to_state = 'available'
                        AND reason_code IS NULL
                    WHEN 'hold_set' THEN actor_kind IN ('admin', 'worker')
                        AND to_state = 'held_off_market'
                        AND reason_code IN ('unpaid_recovered', 'paid_returned', 'seller_ineligible',
                                            'service_disabled', 'quota_blocked', 'operations_hold',
                                            'review')
                    WHEN 'withdrawal_requested' THEN actor_kind = 'public_user'
                        AND to_state = 'deposit_accepted' AND reason_code IS NULL
                    -- 取得后已有撤回意愿时由整封证明直接转撤回待寄；原因必须清空。
                    WHEN 'withdrawal_pending' THEN actor_kind = 'worker'
                        AND to_state = 'withdrawal_pending' AND reason_code IS NULL
                    -- 000137：已在库而挂单尚未上架时的直接撤回，与 available / held_off_market 同一事件。
                    WHEN 'withdrawal_accepted' THEN actor_kind IN ('public_user', 'worker')
                        AND to_state = 'withdrawal_pending' AND reason_code IS NULL
                    WHEN 'season_closed' THEN actor_kind IN ('worker', 'system')
                        AND to_state = 'closed' AND reason_code = 'season_closed'
                END
            WHEN from_state = 'available' THEN
                CASE event_kind
                    WHEN 'repriced' THEN actor_kind = 'public_user'
                        AND to_state = 'available' AND reason_code IS NULL
                    WHEN 'purchased' THEN actor_kind = 'public_user'
                        AND to_state = 'sale_pending' AND reason_code IS NULL
                    WHEN 'hold_set' THEN actor_kind = 'admin' AND to_state = 'held_off_market'
                        AND reason_code IN ('unpaid_recovered', 'paid_returned', 'seller_ineligible',
                                            'service_disabled', 'quota_blocked', 'operations_hold',
                                            'review')
                    WHEN 'withdrawal_requested' THEN actor_kind = 'public_user'
                        AND to_state = 'withdrawal_pending' AND reason_code IS NULL
                    WHEN 'withdrawal_accepted' THEN actor_kind IN ('public_user', 'worker')
                        AND to_state = 'withdrawal_pending' AND reason_code IS NULL
                    WHEN 'loss_confirmed' THEN actor_kind = 'admin' AND to_state = 'closed'
                        AND reason_code = 'loss_confirmed'
                    WHEN 'season_closed' THEN actor_kind IN ('worker', 'system')
                        AND to_state = 'closed' AND reason_code = 'season_closed'
                END
            WHEN from_state = 'held_off_market' THEN
                CASE event_kind
                    WHEN 'hold_released' THEN actor_kind = 'admin'
                        AND to_state = 'held_off_market' AND reason_code = 'review'
                    WHEN 'hold_set' THEN actor_kind = 'admin'
                        AND to_state = 'held_off_market'
                        AND reason_code IN ('unpaid_recovered', 'paid_returned', 'seller_ineligible',
                                            'service_disabled', 'quota_blocked', 'operations_hold',
                                            'review')
                    WHEN 'withdrawal_requested' THEN actor_kind = 'public_user'
                        AND to_state = 'held_off_market'
                        AND reason_code IN ('unpaid_recovered', 'paid_returned', 'seller_ineligible',
                                            'service_disabled', 'quota_blocked', 'operations_hold',
                                            'review')
                    -- 取得后撤回意愿优先于运营止售：由整封证明从下架态转撤回待寄。
                    WHEN 'withdrawal_pending' THEN actor_kind = 'worker'
                        AND to_state = 'withdrawal_pending' AND reason_code IS NULL
                    -- 000137：未付收回后的手动重列、直接撤回与灭失关闭。
                    WHEN 'relisted' THEN actor_kind IN ('public_user', 'worker') AND to_state = 'available'
                        AND reason_code IS NULL
                    WHEN 'withdrawal_accepted' THEN actor_kind IN ('public_user', 'worker')
                        AND to_state = 'withdrawal_pending' AND reason_code IS NULL
                    WHEN 'loss_confirmed' THEN actor_kind = 'admin' AND to_state = 'closed'
                        AND reason_code = 'loss_confirmed'
                    WHEN 'season_closed' THEN actor_kind IN ('worker', 'system')
                        AND to_state = 'closed' AND reason_code = 'season_closed'
                END
            WHEN from_state = 'sale_pending' THEN
                CASE event_kind
                    WHEN 'closed' THEN actor_kind = 'worker'
                        AND to_state = 'closed' AND reason_code = 'sold'
                    -- 7A-2：未付收回处置把挂单转为下架（原因固定 unpaid_recovered）。
                    WHEN 'hold_set' THEN actor_kind = 'worker'
                        AND to_state = 'held_off_market' AND reason_code = 'unpaid_recovered'
                END
            WHEN from_state = 'withdrawal_pending' THEN
                CASE event_kind
                    WHEN 'withdrawn_delivered' THEN actor_kind = 'worker' AND to_state = 'closed'
                        AND reason_code = 'withdrawn_delivered'
                    WHEN 'season_closed' THEN actor_kind IN ('worker', 'system')
                        AND to_state = 'closed' AND reason_code = 'season_closed'
                END
        END
    ) IS TRUE);;

CREATE OR REPLACE FUNCTION public.consignment_check_listing_event() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    previous_state text;
    previous_reason text;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        previous_state := OLD.state;
        previous_reason := OLD.reason_code;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM public.consignment_events AS e
        JOIN public.consignment_commands AS c ON c.command_id = e.command_id
        WHERE e.listing_id = NEW.id AND e.state_version = NEW.state_version
          AND e.from_state IS NOT DISTINCT FROM previous_state AND e.to_state = NEW.state
          AND e.reason_code IS NOT DISTINCT FROM NEW.reason_code AND e.price = NEW.price
          AND e.occurred_at = NEW.updated_at
          AND c.xmin = pg_current_xact_id()::xid
          AND (
            (
                c.schema_version = 1
                AND (
                    (c.listing_id = NEW.id
                      AND (c.result_status >= 400 OR (
                          c.result_body->>'listing_version' = NEW.state_version::text
                          AND c.result_body->>'state' = NEW.state
                          AND (c.result_body->'price' = 'null'::jsonb
                               OR c.result_body->>'price' = NEW.price::text)
                          AND (c.operation NOT IN ('apply_lookup', 'auto_close') OR (
                              (c.result_body->>'expires_at')::timestamptz = NEW.expires_at
                              AND c.result_body->>'reason_code' IS NOT DISTINCT FROM NEW.reason_code
                              AND c.result_body->>'disposition' = e.event_kind
                          ))
                      ))
                      AND CASE e.event_kind
                            WHEN 'registered' THEN c.operation = 'register' AND c.result_status = 202
                                -- 000138：匿名登记的 actor_ref 是原样自填名，挂单没有本站账户。
                                AND CASE c.actor_kind
                                    WHEN 'anonymous' THEN NEW.seller_user_id IS NULL
                                        AND c.actor_ref = NEW.seller_game_name
                                    ELSE c.actor_ref = NEW.seller_user_id::text
                                END
                                AND c.result_body->>'price' = NEW.price::text
                                AND EXISTS (
                                    SELECT 1 FROM public.consignment_replay_keys AS k
                                    WHERE k.id = c.replay_key_id AND k.write_current
                                )
                            WHEN 'repriced' THEN c.operation = 'reprice'
                                AND c.actor_ref = NEW.seller_user_id::text
                                AND c.result_body->>'disposition' = 'repriced'
                            WHEN 'published' THEN c.operation = 'apply_lookup'
                                AND c.result_status = 200
                                AND EXISTS (
                                    SELECT 1 FROM public.consignment_snapshots AS s
                                    WHERE s.id = NEW.current_snapshot_id AND s.tradeable
                                )
                            WHEN 'rejected' THEN c.operation = 'apply_lookup'
                                AND c.result_status = 200
                            WHEN 'expired' THEN
                                c.operation IN ('auto_close', 'apply_lookup')
                                OR c.result_code = 'listing_not_active'
                            WHEN 'closed' THEN CASE e.actor_kind
                                WHEN 'public_user' THEN c.operation = 'withdraw'
                                    AND c.result_status = 200
                                    AND c.actor_ref = NEW.seller_user_id::text
                                WHEN 'admin' THEN c.operation = 'admin_delist'
                                    AND c.result_status = 200
                                WHEN 'system' THEN
                                    c.operation IN ('auto_close', 'apply_lookup')
                                    OR c.result_code = 'listing_not_active'
                                ELSE false END
                            ELSE false END
                    )
                    OR (e.actor_kind = 'system' AND e.event_kind IN ('expired', 'closed')
                        AND c.operation IN ('register', 'apply_lookup')
                        AND c.listing_id IS DISTINCT FROM NEW.id
                        AND (
                            (c.listing_id IS NULL AND c.operation = 'register'
                                AND c.result_status >= 400)
                            OR EXISTS (
                                SELECT 1 FROM public.consignment_listings AS target
                                WHERE target.id = c.listing_id AND target.world_id = NEW.world_id
                                  AND target.external_equipment_id = NEW.external_equipment_id
                            )
                        )
                    )
                )
            )
            OR (e.actor_kind = 'system' AND e.event_kind IN ('expired', 'closed')
                AND c.schema_version = 1
                AND c.operation IN ('register', 'apply_lookup')
                AND c.listing_id IS DISTINCT FROM NEW.id
                AND (
                    (c.listing_id IS NULL AND c.operation = 'register' AND c.result_status >= 400)
                    OR EXISTS (
                        SELECT 1 FROM public.consignment_listings AS target
                        WHERE target.id = c.listing_id AND target.world_id = NEW.world_id
                          AND target.external_equipment_id = NEW.external_equipment_id
                    )
                )
            )
            OR (
                c.schema_version = 2
                AND CASE e.event_kind
                    WHEN 'purchased' THEN
                        c.operation = 'purchase' AND c.actor_kind = 'public_user'
                        AND c.listing_id = NEW.id
                        AND c.result_body->>'listing_id' = NEW.id::text
                        AND c.result_body->>'listing_version' = NEW.state_version::text
                        AND c.result_body->>'state' = NEW.state
                        AND c.result_body->>'disposition' = 'purchased'
                        AND NEW.reason_code IS NULL
                        AND EXISTS (
                            SELECT 1 FROM public.consignment_contracts AS contract
                            WHERE contract.contract_id = (c.result_body->>'contract_id')::uuid
                              AND contract.listing_id = NEW.id AND contract.kind = 'sale'
                              AND contract.origin_command_id = c.command_id
                        )
                    WHEN 'repriced' THEN
                        c.operation = 'reprice' AND c.actor_kind = 'public_user'
                        AND c.listing_id = NEW.id AND c.actor_ref = NEW.seller_user_id::text
                        AND c.result_body->>'listing_id' = NEW.id::text
                        AND c.result_body->>'listing_version' = NEW.state_version::text
                        AND c.result_body->>'state' = NEW.state
                        AND c.result_body->>'price' = NEW.price::text
                        AND c.result_body->>'disposition' = 'repriced'
                    WHEN 'reservation_issued' THEN
                        c.operation = 'issue_reservation' AND c.actor_kind = 'public_user'
                        AND c.listing_id = NEW.id AND c.actor_ref = NEW.seller_user_id::text
                        AND NEW.reason_code IS NOT DISTINCT FROM previous_reason
                    WHEN 'reservation_retired' THEN
                        c.operation = 'expire_reservation' AND c.actor_kind = 'worker'
                        AND c.target_kind = 'reservation'
                        AND c.target_id = (c.result_body->>'reservation_id')::uuid
                        AND c.result_body->>'disposition' = 'reservation_retired'
                        AND NEW.reason_code IS NOT DISTINCT FROM previous_reason
                        AND EXISTS (
                            SELECT 1 FROM public.consignment_deposit_reservations AS reservation
                            WHERE reservation.id = c.target_id
                              AND reservation.listing_id = NEW.id
                              AND reservation.state = 'retired'
                              AND reservation.last_command_id = c.command_id
                        )
                    WHEN 'deposit_accepted' THEN
                        ((c.operation = 'decide_envelope' AND c.actor_kind = 'worker')
                         OR (c.operation = 'release_quarantine'
                             AND c.actor_kind IN ('admin', 'system')))
                        AND EXISTS (
                            SELECT 1 FROM jsonb_array_elements(
                                COALESCE(c.result_body->'listing_changes', '[]'::jsonb)) AS change
                            WHERE change->>'listing_id' = NEW.id::text
                              AND change->>'listing_version' = NEW.state_version::text
                              AND change->>'state' = NEW.state
                        )
                    WHEN 'available' THEN
                        c.operation = 'apply_mail_proof' AND c.actor_kind = 'worker'
                        AND EXISTS (
                            SELECT 1 FROM jsonb_array_elements(
                                COALESCE(c.result_body->'listing_changes', '[]'::jsonb)) AS change
                            WHERE change->>'listing_id' = NEW.id::text
                              AND change->>'listing_version' = NEW.state_version::text
                              AND change->>'state' = NEW.state
                        )
                    WHEN 'withdrawal_pending' THEN
                        c.operation = 'apply_mail_proof' AND c.actor_kind = 'worker'
                        AND EXISTS (
                            SELECT 1 FROM jsonb_array_elements(
                                COALESCE(c.result_body->'listing_changes', '[]'::jsonb)) AS change
                            WHERE change->>'listing_id' = NEW.id::text
                              AND change->>'listing_version' = NEW.state_version::text
                              AND change->>'state' = NEW.state
                        )
                        AND EXISTS (
                            SELECT 1 FROM jsonb_array_elements(
                                COALESCE(c.result_body->'custody_changes', '[]'::jsonb)) AS custody
                            WHERE custody->>'listing_id' = NEW.id::text
                              AND custody->>'listing_version' = NEW.state_version::text
                              AND custody->>'state' = NEW.state
                              AND custody->>'held_reason' IS NULL
                        )
                    WHEN 'hold_set' THEN
                        (
                            (c.operation = 'admin_delist' AND c.actor_kind = 'admin'
                             AND c.listing_id = NEW.id
                             AND c.result_body->>'listing_version' = NEW.state_version::text
                             AND c.result_body->>'state' = NEW.state
                             AND c.result_body->>'disposition' = 'hold_set'
                             AND NEW.reason_code = 'operations_hold'
                             AND c.result_body->>'held_reason' = NEW.reason_code)
                            OR
                            (c.operation = 'apply_mail_proof' AND c.actor_kind = 'worker'
                             AND NEW.reason_code IN (
                                 'review', 'operations_hold', 'seller_ineligible',
                                 'service_disabled', 'quota_blocked')
                             AND EXISTS (
                                 SELECT 1 FROM jsonb_array_elements(
                                     COALESCE(c.result_body->'listing_changes', '[]'::jsonb)) AS change
                                 WHERE change->>'listing_id' = NEW.id::text
                                   AND change->>'listing_version' = NEW.state_version::text
                                   AND change->>'state' = NEW.state
                             )
                             AND EXISTS (
                                 SELECT 1 FROM jsonb_array_elements(
                                     COALESCE(c.result_body->'custody_changes', '[]'::jsonb)) AS custody
                                 WHERE custody->>'listing_id' = NEW.id::text
                                   AND custody->>'listing_version' = NEW.state_version::text
                                   AND custody->>'state' = NEW.state
                                   AND custody->>'held_reason' = NEW.reason_code
                             ))
                            OR
                            -- 7A-2：未付收回处置把成交待履约的挂单转为下架；回执带本挂单的变更，
                            -- 同事务的处置行与封闭事实由挂单守卫核对。
                            (c.operation = 'resolve_return' AND c.actor_kind = 'worker'
                             AND e.actor_kind = 'worker'
                             AND NEW.reason_code = 'unpaid_recovered'
                             AND EXISTS (
                                 SELECT 1 FROM jsonb_array_elements(
                                     COALESCE(c.result_body->'listing_changes', '[]'::jsonb)) AS change
                                 WHERE change->>'listing_id' = NEW.id::text
                                   AND change->>'listing_version' = NEW.state_version::text
                                   AND change->>'state' = NEW.state
                                   AND change->>'hold_reason' = NEW.reason_code
                             ))
                        )
                    WHEN 'hold_released' THEN
                        c.operation = 'release_hold' AND c.actor_kind = 'admin'
                        AND c.listing_id = NEW.id
                        AND c.result_body->>'listing_version' = NEW.state_version::text
                        AND c.result_body->>'state' = NEW.state
                        AND c.result_body->>'disposition' = 'hold_released'
                        AND NEW.reason_code = 'review'
                        AND c.result_body->>'held_reason' = NEW.reason_code
                    WHEN 'withdrawal_requested' THEN
                        c.operation = 'withdraw' AND c.actor_kind = 'public_user'
                        AND c.listing_id = NEW.id
                        -- 000138：与 'closed' 同口径的归属门，纵深防御。匿名挂单只能是非寄售
                        -- （consignment_listing_seller），而撤回意愿只发生在代管态，目前不可达；
                        -- 构造反例须先放宽挂单约束或状态守卫，故不单设数据库用例。
                        AND (NEW.seller_user_id IS NOT NULL OR EXISTS (
                            SELECT 1 FROM public.users AS same_name
                            WHERE same_name.id::text = c.actor_ref
                              AND lower(same_name.username) = lower(NEW.seller_game_name)
                        ))
                        AND c.result_body->>'listing_id' = NEW.id::text
                        AND c.result_body->>'listing_version' = NEW.state_version::text
                        AND c.result_body->>'state' = NEW.state
                        AND c.result_body->>'disposition' = 'withdrawal_requested'
                        AND NEW.reason_code IS NOT DISTINCT FROM previous_reason
                    WHEN 'closed' THEN
                        (
                            (
                                (c.operation = 'withdraw' AND c.actor_kind = 'public_user'
                                 AND c.listing_id = NEW.id
                                 -- 000138：匿名挂单只允许与自填名 lower 等值的本站账户撤回。
                                 AND (NEW.seller_user_id IS NOT NULL OR EXISTS (
                                     SELECT 1 FROM public.users AS same_name
                                     WHERE same_name.id::text = c.actor_ref
                                       AND lower(same_name.username) = lower(NEW.seller_game_name)
                                 )))
                                OR
                                (c.operation = 'admin_delist' AND c.actor_kind = 'admin'
                                 AND c.listing_id = NEW.id)
                            )
                            AND c.result_body->>'listing_id' = NEW.id::text
                            AND c.result_body->>'listing_version' = NEW.state_version::text
                            AND c.result_body->>'state' = NEW.state
                            AND c.result_body->>'disposition' = 'closed'
                        )
                        OR
                        (
                            -- 买家确领原件关闭成交（000136）：回执带本挂单的版本与状态。
                            c.operation = 'prove_handoff' AND c.actor_kind = 'worker'
                            AND NEW.reason_code = 'sold'
                            AND c.result_body->>'listing_id' = NEW.id::text
                            AND c.result_body->>'listing_version' = NEW.state_version::text
                            AND c.result_body->>'state' = NEW.state
                            AND c.result_body->>'disposition' = 'original_claimed'
                        )
                    -- ===== 以下四个事件为 000137 新增（重列、直接撤回、灭失、撤回交付）。 =====
                    WHEN 'relisted' THEN
                        -- 手动重列由原卖家发起；自动重列（7A-2）是 worker 的 auto_relist 命令。
                        ((c.operation = 'relist' AND c.actor_kind = 'public_user'
                          AND c.actor_ref = NEW.seller_user_id::text)
                         OR (c.operation = 'auto_relist' AND c.actor_kind = 'worker'))
                        AND e.actor_kind = c.actor_kind
                        AND c.listing_id = NEW.id
                        AND c.result_body->>'listing_id' = NEW.id::text
                        AND c.result_body->>'listing_version' = NEW.state_version::text
                        AND c.result_body->>'state' = NEW.state
                        AND c.result_body->>'price' = NEW.price::text
                        AND c.result_body->>'disposition' = 'relisted'
                        AND NEW.reason_code IS NULL
                        -- 重列必须同事务追加 g+1 重列代，且回执的 generation 即该代。
                        AND EXISTS (
                            SELECT 1 FROM public.consignment_disposition_generations AS successor
                            JOIN public.consignment_custody_positions AS position_row
                              ON position_row.id = successor.position_id
                            WHERE position_row.listing_id = NEW.id
                              AND successor.purpose = 'relist'
                              AND successor.generation::text = c.result_body->>'generation'
                              AND successor.created_command_id = c.command_id
                              AND successor.xmin = pg_current_xact_id()::xid
                        )
                    WHEN 'withdrawal_accepted' THEN
                        (
                            (c.operation = 'withdraw' AND c.actor_kind = 'public_user'
                             AND c.actor_ref = NEW.seller_user_id::text)
                            OR (c.operation = 'fulfill_withdraw_intent' AND c.actor_kind = 'worker')
                        )
                        AND c.listing_id = NEW.id
                        AND c.result_body->>'listing_id' = NEW.id::text
                        AND c.result_body->>'listing_version' = NEW.state_version::text
                        AND c.result_body->>'state' = NEW.state
                        AND c.result_body->>'disposition' = 'withdrawal_accepted'
                        AND NEW.reason_code IS NULL
                        -- 同事务建立的 Withdrawal 合同（来源命令即本命令）。
                        AND EXISTS (
                            SELECT 1 FROM public.consignment_contracts AS contract
                            WHERE contract.contract_id = (c.result_body->>'contract_id')::uuid
                              AND contract.listing_id = NEW.id AND contract.kind = 'withdrawal'
                              AND contract.origin_command_id = c.command_id
                        )
                    WHEN 'loss_confirmed' THEN
                        c.operation = 'confirm_position_loss' AND c.actor_kind = 'admin'
                        AND c.result_body->>'listing_id' = NEW.id::text
                        AND c.result_body->>'listing_version' = NEW.state_version::text
                        AND c.result_body->>'state' = NEW.state
                        AND c.result_body->>'disposition' = 'position_loss_recorded'
                        AND NEW.reason_code = 'loss_confirmed'
                        AND EXISTS (
                            SELECT 1 FROM public.consignment_loss_facts AS fact
                            WHERE fact.loss_fact_id = (c.result_body->>'loss_fact_id')::uuid
                              AND fact.listing_id = NEW.id AND fact.origin_command_id = c.command_id
                        )
                    WHEN 'withdrawn_delivered' THEN
                        -- 合同未指明交付证明命令：沿成交确领的 worker 命令 prove_handoff（original_claimed）
                        -- 取最保守绑定，并要求同事务的 consignment_withdrawn 交接边（挂单守卫已查）。
                        c.operation = 'prove_handoff' AND c.actor_kind = 'worker'
                        AND NEW.reason_code = 'withdrawn_delivered'
                        AND c.result_body->>'listing_id' = NEW.id::text
                        AND c.result_body->>'listing_version' = NEW.state_version::text
                        AND c.result_body->>'state' = NEW.state
                        AND c.result_body->>'disposition' = 'original_claimed'
                    WHEN 'season_closed' THEN
                        c.operation = 'season_close' AND c.actor_kind = 'worker'
                        AND c.target_kind = 'season'
                        AND EXISTS (
                            SELECT 1 FROM jsonb_array_elements_text(
                                COALESCE(c.result_body->'closed_listing_ids', '[]'::jsonb))
                                AS closed(value)
                            WHERE value = NEW.id::text
                        )
                    ELSE false END
            )
          )
    ) THEN
        RAISE EXCEPTION '寄售挂单版本缺少同事务的匹配事件或命令'
            USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_version_event';
    END IF;
    RETURN NULL;
END
$function$;

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (138, 'breaking');
