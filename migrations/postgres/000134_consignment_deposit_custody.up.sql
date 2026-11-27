-- 寄售第 4 期：收件、整封决定、隔离、代管实物与旧 owner 交接的全部持久结构。
-- owner: auction-storage（寄售收件与代管、维护授权）
-- 不变量 1、3、5、6、7、8、10：命令与事实只追加；整封接受在提交前一一对应；
-- 同一实物只有一份强占用；旧拍卖路由先运行且行为不变；秘密只以信封保存。
-- 等级：breaking。新增挂单状态/期限、lot 准入守卫、投递来源与整封提交校验均改变旧程序的
-- 写入与回退语义，须在停机门禁下与同版程序一起切换。

LOCK TABLE
    public.consignment_listings,
    public.consignment_commands,
    public.consignment_events,
    public.inbound_mail,
    public.deliveries,
    public.lots,
    public.equipment,
    public.settlement_return_cases,
    public.settlement_return_evidence,
    public.settlement_recoveries,
    public.settlement_obligations,
    public.mail_poll_pending_reads,
    public.credential_keys
IN SHARE ROW EXCLUSIVE MODE;

-- 恢复详情候选时必须原样还原列表投影；旧版登记的原始分钟为 NULL，门控关闭时仍按原方式恢复，
-- 门控打开时由读取器在缺少原始分钟且无法从列表补证时保留 pending。世界标记本身可为 NULL。
ALTER TABLE public.mail_poll_pending_reads
    ADD COLUMN list_world_marker text,
    ADD COLUMN received_minute_raw text,
    ADD CONSTRAINT mail_poll_pending_reads_list_world_marker_ck
        CHECK (list_world_marker IS NULL OR list_world_marker IN ('main', 'isekai')),
    ADD CONSTRAINT mail_poll_pending_reads_received_minute_raw_ck
        CHECK (received_minute_raw IS NULL
               OR public.consignment_valid_text(received_minute_raw, 64)),
    ADD CONSTRAINT mail_poll_pending_reads_list_snapshot_ck
        CHECK (list_world_marker IS NULL OR received_minute_raw IS NOT NULL);

-- 非零 UUID、摘要、闭集文本与数组的共用谓词。所有表级 CHECK 均显式命名，
-- 应用错误映射只依赖 SQLSTATE 与约束名。
CREATE FUNCTION public.c4_valid_uuid(value uuid) RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog
AS $function$
    SELECT value <> uuid '00000000-0000-0000-0000-000000000000'
$function$;

CREATE FUNCTION public.c4_valid_uuid_text(value text) RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog
AS $function$
    SELECT public.consignment_positive_uuid(value)
$function$;

CREATE FUNCTION public.c4_valid_digest(value bytea) RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog
AS $function$
    SELECT octet_length(value) = 32
$function$;

CREATE FUNCTION public.c4_valid_uuid_array(entries uuid[]) RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog
AS $function$
    SELECT CASE WHEN array_ndims(entries) > 1 THEN false
                ELSE array_position(entries, NULL) IS NULL END
$function$;

CREATE FUNCTION public.c4_valid_text_array(entries text[], maximum_items integer, maximum_length integer)
RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog, public
AS $function$
    SELECT cardinality(entries) <= maximum_items
       AND array_position(entries, NULL) IS NULL
       AND COALESCE((
           SELECT bool_and(public.consignment_valid_text(value, maximum_length))
           FROM unnest(entries) AS value
       ), true)
$function$;

CREATE FUNCTION public.c4_valid_json_uuid(value jsonb) RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog, public
AS $function$
    SELECT jsonb_typeof(value) = 'string' AND public.c4_valid_uuid_text(value #>> '{}')
$function$;

CREATE FUNCTION public.c4_valid_json_time(value jsonb) RETURNS boolean
LANGUAGE sql STABLE STRICT PARALLEL SAFE SET search_path = pg_catalog, public
AS $function$
    SELECT jsonb_typeof(value) = 'string' AND public.consignment_result_time(value #>> '{}')
$function$;

CREATE FUNCTION public.c4_valid_json_version(value jsonb) RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog
AS $function$
    SELECT jsonb_typeof(value) = 'number'
       AND (value #>> '{}') COLLATE "C" ~ '^[1-9][0-9]{0,15}$'
       AND (value #>> '{}')::numeric <= 9007199254740991
$function$;

CREATE FUNCTION public.c4_valid_json_digest(value jsonb) RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog
AS $function$
    SELECT jsonb_typeof(value) = 'string'
       AND (value #>> '{}') COLLATE "C" ~ '^[a-f0-9]{64}$'
$function$;

-- 观察清单是类型化 JSON 数组：序号唯一、有界，装备只保存规范 EID 或受保护引用，
-- 不保存原始装备 URL、key 或自由正文。
CREATE FUNCTION public.c4_valid_manifest(manifest jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog, public
AS $function$
DECLARE
    item jsonb;
    ordinal_value bigint;
    kind_value text;
    seen bigint[] := ARRAY[]::bigint[];
BEGIN
    IF jsonb_typeof(manifest) <> 'array' OR jsonb_array_length(manifest) > 200 THEN
        RETURN false;
    END IF;
    FOR item IN SELECT value FROM jsonb_array_elements(manifest) LOOP
        IF jsonb_typeof(item) <> 'object' THEN RETURN false; END IF;
        IF NOT (item - ARRAY['ordinal', 'kind', 'name', 'raw_eid', 'canonical_eid', 'amount',
                             'protected_ref']
                = '{}'::jsonb) IS TRUE THEN RETURN false; END IF;
        IF jsonb_typeof(item->'ordinal') <> 'number' THEN RETURN false; END IF;
        IF (item->>'ordinal') COLLATE "C" !~ '^[1-9][0-9]{0,3}$' THEN RETURN false; END IF;
        ordinal_value := (item->>'ordinal')::bigint;
        IF ordinal_value > 200 OR ordinal_value = ANY(seen) THEN RETURN false; END IF;
        seen := array_append(seen, ordinal_value);
        IF jsonb_typeof(item->'kind') <> 'string' THEN RETURN false; END IF;
        kind_value := item->>'kind';
        IF kind_value NOT IN ('equipment', 'credits', 'cod', 'unknown') THEN RETURN false; END IF;
        IF item ? 'raw_eid' AND jsonb_typeof(item->'raw_eid') <> 'null'
           AND NOT public.consignment_safe_observation_text(item->>'raw_eid', 64) THEN
            RETURN false;
        END IF;
        IF item ? 'name' AND jsonb_typeof(item->'name') <> 'null'
           AND NOT public.consignment_safe_observation_text(item->>'name', 256) THEN
            RETURN false;
        END IF;
        IF item ? 'protected_ref' AND jsonb_typeof(item->'protected_ref') <> 'null'
           AND NOT public.consignment_safe_observation_text(item->>'protected_ref', 512) THEN
            RETURN false;
        END IF;
        IF kind_value = 'equipment' THEN
            IF jsonb_typeof(item->'canonical_eid') <> 'string'
               OR (item->>'canonical_eid') COLLATE "C" !~ '^[1-9][0-9]{0,31}$' THEN
                RETURN false;
            END IF;
            IF item ? 'amount' AND jsonb_typeof(item->'amount') <> 'null' THEN RETURN false; END IF;
        ELSIF kind_value IN ('credits', 'cod') THEN
            IF jsonb_typeof(item->'amount') <> 'number'
               OR (item->>'amount') COLLATE "C" !~ '^[0-9]{1,10}$'
               OR (item->>'amount')::numeric > 1000000000 THEN
                RETURN false;
            END IF;
            IF item ? 'canonical_eid' AND jsonb_typeof(item->'canonical_eid') <> 'null' THEN
                RETURN false;
            END IF;
        END IF;
    END LOOP;
    RETURN true;
END
$function$;

-- 观察行中无法由其他列表达、且观察时即确定的派生事实。键集合闭集，类型与取值逐项校验；
-- 只保留规范系统主题（用于补写旧格式入站行），不保存普通原文主题、发件人显示名之外的
-- 资格文本或任何秘密。
CREATE FUNCTION public.c4_valid_derived_facts(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog, public
AS $function$
DECLARE
    item jsonb;
    ordinal_value bigint;
    seen bigint[] := ARRAY[]::bigint[];
BEGIN
    IF jsonb_typeof(value) <> 'object' THEN RETURN false; END IF;
    IF NOT (value ?& ARRAY['subject_is_consign', 'sender_identity', 'list_detail_consistent',
                           'duplicate_raw_eid', 'canonical_alias_ambiguous',
                           'unknown_attachment', 'system_rts', 'legacy_auction_owner',
                           'received_minute_raw', 'items'])
       OR NOT (value - ARRAY['subject_is_consign', 'sender_identity', 'list_detail_consistent',
                             'duplicate_raw_eid', 'canonical_alias_ambiguous',
                             'unknown_attachment', 'system_rts', 'legacy_auction_owner',
                             'legacy_obligation_id', 'legacy_subject', 'returned_from', 'body_digest',
                             'dispatch_reference',
                             'received_minute_raw', 'items', 'asset_count'] = '{}'::jsonb) IS TRUE THEN
        RETURN false;
    END IF;
    IF jsonb_typeof(value->'subject_is_consign') <> 'boolean'
       OR jsonb_typeof(value->'list_detail_consistent') <> 'boolean'
       OR jsonb_typeof(value->'duplicate_raw_eid') <> 'boolean'
       OR jsonb_typeof(value->'canonical_alias_ambiguous') <> 'boolean'
       OR jsonb_typeof(value->'unknown_attachment') <> 'boolean'
       OR jsonb_typeof(value->'system_rts') <> 'boolean' THEN
        RETURN false;
    END IF;
    IF jsonb_typeof(value->'sender_identity') <> 'string'
       OR (value->>'sender_identity') NOT IN
          ('verified', 'same_name_different_uid', 'unverified') THEN
        RETURN false;
    END IF;
    IF jsonb_typeof(value->'legacy_auction_owner') <> 'null'
       AND (jsonb_typeof(value->'legacy_auction_owner') <> 'string'
            OR (value->>'legacy_auction_owner') NOT IN (
                'seller_payout', 'buyer_refund', 'unsold_return', 'withdrawn_return',
                'winner_equipment_cod', 'winner_payment_carrier_cod',
                'winner_equipment_handoff')) THEN
        RETURN false;
    END IF;
    IF value ? 'legacy_obligation_id'
       AND NOT (public.c4_valid_json_uuid(value->'legacy_obligation_id')
                OR jsonb_typeof(value->'legacy_obligation_id') = 'null') THEN
        RETURN false;
    END IF;
    IF value ? 'legacy_subject' AND jsonb_typeof(value->'legacy_subject') <> 'null' THEN
        IF jsonb_typeof(value->'legacy_obligation_id') IS DISTINCT FROM 'string'
           OR jsonb_typeof(value->'legacy_subject') <> 'string'
           OR NOT public.consignment_safe_observation_text(value->>'legacy_subject', 512) THEN
            RETURN false;
        END IF;
    END IF;
    IF value ? 'returned_from' AND jsonb_typeof(value->'returned_from') <> 'null' THEN
        IF jsonb_typeof(value->'returned_from') <> 'string'
           OR NOT public.consignment_safe_observation_text(value->>'returned_from', 256) THEN
            RETURN false;
        END IF;
    END IF;
    IF value ? 'body_digest' AND jsonb_typeof(value->'body_digest') <> 'null' THEN
        IF jsonb_typeof(value->'body_digest') <> 'string'
           OR (value->>'body_digest') COLLATE "C" !~ '^[a-f0-9]{64}$' THEN
            RETURN false;
        END IF;
    END IF;
    IF value ? 'dispatch_reference' AND jsonb_typeof(value->'dispatch_reference') <> 'null' THEN
        IF jsonb_typeof(value->'dispatch_reference') <> 'string'
           OR NOT public.consignment_safe_observation_text(
                  value->>'dispatch_reference', 128) THEN
            RETURN false;
        END IF;
    END IF;
    IF jsonb_typeof(value->'received_minute_raw') <> 'string'
       OR NOT public.consignment_safe_observation_text(
              value->>'received_minute_raw', 64) THEN
        RETURN false;
    END IF;
    IF value ? 'asset_count' AND jsonb_typeof(value->'asset_count') <> 'null' THEN
        IF jsonb_typeof(value->'asset_count') <> 'number'
           OR (value->>'asset_count') COLLATE "C" !~ '^[0-9]{1,3}$'
           OR (value->>'asset_count')::numeric > 200 THEN
            RETURN false;
        END IF;
    END IF;
    IF jsonb_typeof(value->'items') <> 'array'
       OR jsonb_array_length(value->'items') > 200 THEN
        RETURN false;
    END IF;
    FOR item IN SELECT entry FROM jsonb_array_elements(value->'items') AS entry LOOP
        IF jsonb_typeof(item) <> 'object' THEN RETURN false; END IF;
        IF NOT (item ?& ARRAY['ordinal', 'world', 'tradeable', 'history_ok'])
           OR NOT (item - ARRAY['ordinal', 'world', 'tradeable', 'history_ok']
                = '{}'::jsonb) IS TRUE THEN RETURN false; END IF;
        IF jsonb_typeof(item->'ordinal') <> 'number'
           OR (item->>'ordinal') COLLATE "C" !~ '^[1-9][0-9]{0,3}$' THEN
            RETURN false;
        END IF;
        ordinal_value := (item->>'ordinal')::bigint;
        IF ordinal_value > 200 OR ordinal_value = ANY(seen) THEN RETURN false; END IF;
        seen := array_append(seen, ordinal_value);
        IF jsonb_typeof(item->'world') <> 'string'
           OR (item->>'world') NOT IN ('main', 'isekai') THEN
            RETURN false;
        END IF;
        IF jsonb_typeof(item->'tradeable') <> 'boolean'
           OR jsonb_typeof(item->'history_ok') <> 'boolean' THEN
            RETURN false;
        END IF;
    END LOOP;
    RETURN true;
END
$function$;

-- 回执中的逐件 L 数组：listing_id、listing_version、state 必有；止售与撤回字段可选。
CREATE FUNCTION public.c4_valid_listing_changes(changes jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog, public
AS $function$
DECLARE
    item jsonb;
    seen uuid[] := ARRAY[]::uuid[];
    listing uuid;
BEGIN
    IF jsonb_typeof(changes) <> 'array'
       OR jsonb_array_length(changes) NOT BETWEEN 1 AND 20 THEN
        RETURN false;
    END IF;
    FOR item IN SELECT value FROM jsonb_array_elements(changes) LOOP
        IF jsonb_typeof(item) <> 'object' THEN RETURN false; END IF;
        IF NOT (item - ARRAY['listing_id', 'listing_version', 'state', 'hold_reason',
                             'hold_id', 'withdrawal_intent_id'] = '{}'::jsonb) IS TRUE THEN
            RETURN false;
        END IF;
        IF NOT public.c4_valid_json_uuid(item->'listing_id') THEN RETURN false; END IF;
        listing := (item->>'listing_id')::uuid;
        IF listing = ANY(seen) THEN RETURN false; END IF;
        seen := array_append(seen, listing);
        IF NOT public.c4_valid_json_version(item->'listing_version') THEN RETURN false; END IF;
        IF jsonb_typeof(item->'state') <> 'string'
           OR item->>'state' NOT IN ('resolving', 'displayed', 'awaiting_deposit',
                                     'deposit_accepted', 'available', 'sale_pending',
                                     'held_off_market', 'withdrawal_pending',
                                     'rejected', 'expired', 'closed') THEN
            RETURN false;
        END IF;
        IF item ? 'hold_reason' AND jsonb_typeof(item->'hold_reason') <> 'null'
           AND item->>'hold_reason' NOT IN ('unpaid_recovered', 'paid_returned',
                                            'seller_ineligible', 'service_disabled',
                                            'quota_blocked', 'operations_hold', 'review') THEN
            RETURN false;
        END IF;
        IF item ? 'hold_id' AND jsonb_typeof(item->'hold_id') <> 'null'
           AND NOT public.c4_valid_json_uuid(item->'hold_id') THEN RETURN false; END IF;
        IF item ? 'withdrawal_intent_id' AND jsonb_typeof(item->'withdrawal_intent_id') <> 'null'
           AND NOT public.c4_valid_json_uuid(item->'withdrawal_intent_id') THEN RETURN false; END IF;
    END LOOP;
    RETURN true;
END
$function$;

-- v2 回执只接受第 4 期操作与状态码；未知字段、错误 operation/command_id 或缺少分支必填字段
-- 都在写入账本时失败关闭。请求形状错误的 400 码、404 command_not_found 与 503
-- consignment_unavailable 不进入本函数：前者确定未写入，后者不属于任何确定闭集。
CREATE FUNCTION public.c4_valid_v2_receipt(
    body jsonb, command_id uuid, operation text, status integer
) RETURNS boolean
LANGUAGE plpgsql STABLE PARALLEL SAFE SET search_path = pg_catalog, public
AS $function$
DECLARE
    disposition text;
BEGIN
    IF jsonb_typeof(body) <> 'object' THEN RETURN false; END IF;
    IF body->'schema_version' IS DISTINCT FROM '2'::jsonb
       OR body->'command_id' IS DISTINCT FROM to_jsonb(command_id)
       OR body->>'operation' IS DISTINCT FROM operation THEN
        RETURN false;
    END IF;
    IF jsonb_typeof(body->'disposition') <> 'string' THEN RETURN false; END IF;
    disposition := body->>'disposition';
    IF status >= 400 THEN
        IF disposition <> 'rejected' THEN RETURN false; END IF;
        -- command_id_conflict 不入账：同 ID 异参冲突不写任何事实。
        IF NOT (
            (status = 403 AND body->>'code' IN ('participant_ineligible', 'bot_party', 'action_not_allowed'))
            OR (status = 404 AND body->>'code' = 'consignment_disabled')
            OR (status = 409 AND body->>'code' IN (
                'stale_version', 'listing_not_active', 'equipment_busy', 'equipment_committed',
                'custody_unproven', 'operations_hold', 'season_closed', 'hold_mismatch',
                'source_mismatch', 'evidence_unproven', 'evidence_conflict',
                'disposition_conflict', 'fence_mismatch'))
            OR (status = 429 AND body->>'code' = 'listing_limit_reached')
            OR (status = 503 AND body->>'code' = 'mail_unavailable')
        ) THEN RETURN false; END IF;
        RETURN true;
    END IF;
    CASE operation
        WHEN 'issue_reservation' THEN
            RETURN status = 201 AND disposition = 'reservation_issued'
                AND public.c4_valid_json_uuid(body->'listing_id')
                AND public.c4_valid_json_version(body->'listing_version')
                AND body->>'state' = 'awaiting_deposit'
                AND public.c4_valid_json_uuid(body->'reservation_id')
                AND public.c4_valid_json_time(body->'expires_at');
        WHEN 'expire_reservation' THEN
            RETURN status = 200 AND disposition = 'reservation_retired'
                AND public.c4_valid_json_uuid(body->'reservation_id');
        WHEN 'decide_envelope' THEN
            IF NOT public.c4_valid_json_uuid(body->'decision_id') THEN RETURN false; END IF;
            IF disposition = 'accepted' THEN
                RETURN status = 200 AND public.c4_valid_listing_changes(body->'listing_changes')
                    AND (NOT body ? 'quarantine' OR body->'quarantine' = 'null'::jsonb);
            ELSIF disposition = 'quarantined' THEN
                RETURN status = 200
                    AND jsonb_typeof(body->'quarantine') = 'object'
                    AND public.c4_valid_json_uuid(body->'quarantine'->'quarantine_id')
                    AND public.c4_valid_json_version(body->'quarantine'->'version')
                    AND (NOT body ? 'listing_changes' OR body->'listing_changes' = '[]'::jsonb);
            ELSE
                RETURN status = 200 AND disposition IN ('return_decided', 'archived_empty')
                    AND (NOT body ? 'listing_changes' OR body->'listing_changes' = '[]'::jsonb)
                    AND (NOT body ? 'quarantine' OR body->'quarantine' = 'null'::jsonb);
            END IF;
        WHEN 'append_quarantine_observation' THEN
            RETURN status = 200 AND disposition = 'observation_appended'
                AND public.c4_valid_json_uuid(body->'quarantine_id')
                AND public.c4_valid_json_version(body->'quarantine_version')
                AND public.c4_valid_json_uuid(body->'observation_id')
                AND public.c4_valid_json_digest(body->'digest');
        WHEN 'recheck_quarantine' THEN
            RETURN status = 202 AND disposition = 'recheck_queued'
                AND public.c4_valid_json_uuid(body->'quarantine_id')
                AND public.c4_valid_json_uuid(body->'work_id');
        WHEN 'release_quarantine' THEN
            RETURN status = 200 AND disposition = 'quarantine_released'
                AND public.c4_valid_json_uuid(body->'quarantine_id')
                AND public.c4_valid_json_uuid(body->'release_id')
                AND public.c4_valid_json_uuid(body->'decision_id')
                AND body->>'decision_kind' IN (
                    'accept', 'return', 'archive_empty', 'acquire_return',
                    'auction_return_case', 'auction_recovery_return')
                AND CASE body->>'decision_kind'
                    WHEN 'accept' THEN public.c4_valid_listing_changes(body->'listing_changes')
                    WHEN 'return' THEN true
                    WHEN 'archive_empty' THEN true
                    WHEN 'acquire_return' THEN
                        public.c4_valid_json_uuid(body->'return_intake_id')
                        AND public.c4_valid_json_version(body->'original_round')
                        AND public.c4_valid_json_uuid(body->'take_work_id')
                        AND public.c4_valid_json_uuid(body->'take_delivery_id')
                    WHEN 'auction_return_case' THEN
                        public.c4_valid_json_uuid(body->'return_mail_id')
                        AND public.c4_valid_json_uuid(body->'return_evidence_id')
                        AND (body->'obligation_id' = 'null'::jsonb
                             OR public.c4_valid_json_uuid(body->'obligation_id'))
                    WHEN 'auction_recovery_return' THEN
                        public.c4_valid_json_uuid(body->'obligation_id')
                        AND public.c4_valid_json_uuid(body->'return_mail_id')
                        AND public.c4_valid_json_uuid(body->'recovery_id')
                        AND public.c4_valid_json_uuid(body->'recovery_handoff_id')
                    ELSE false END;
        WHEN 'start_disposition_write' THEN
            RETURN status = 200 AND disposition = 'write_started'
                AND public.c4_valid_json_uuid(body->'decision_id')
                AND public.c4_valid_json_uuid(body->'work_id')
                AND public.c4_valid_json_uuid(body->'attempt_id')
                AND public.c4_valid_json_version(body->'fence');
        WHEN 'apply_mail_proof' THEN
            RETURN status = 200
                AND disposition IN ('take_proven', 'return_proven', 'evidence_quarantined')
                AND public.c4_valid_json_uuid(body->'decision_id')
                AND public.c4_valid_json_uuid(body->'proof_id');
        WHEN 'withdraw' THEN
            IF disposition = 'closed' THEN
                -- 未代管件的关闭回执：状态必须为 closed，且不得携带代管意愿。
                RETURN status = 200
                    AND public.c4_valid_json_uuid(body->'listing_id')
                    AND public.c4_valid_json_version(body->'listing_version')
                    AND body->>'state' = 'closed'
                    AND NOT body ? 'intent_id';
            END IF;
            -- 代管件的意愿回执：available 已由挂单守卫转为 withdrawal_pending，
            -- deposit_accepted 与 held_off_market 保留原态。
            RETURN status = 202 AND disposition = 'withdrawal_requested'
                AND public.c4_valid_json_uuid(body->'listing_id')
                AND public.c4_valid_json_version(body->'listing_version')
                AND body->>'state' IN (
                    'deposit_accepted', 'withdrawal_pending', 'held_off_market')
                AND public.c4_valid_json_uuid(body->'intent_id');
        WHEN 'admin_delist' THEN
            IF disposition = 'closed' THEN
                RETURN status = 200 AND public.c4_valid_json_uuid(body->'listing_id')
                    AND public.c4_valid_json_version(body->'listing_version')
                    AND body->>'state' = 'closed';
            ELSIF disposition = 'hold_set' THEN
                RETURN status = 200 AND public.c4_valid_json_uuid(body->'listing_id')
                    AND public.c4_valid_json_version(body->'listing_version')
                    AND body->>'state' = 'held_off_market'
                    AND public.c4_valid_json_uuid(body->'hold_id');
            END IF;
            RETURN false;
        WHEN 'release_hold' THEN
            RETURN status = 200 AND disposition = 'hold_released'
                AND public.c4_valid_json_uuid(body->'listing_id')
                AND public.c4_valid_json_version(body->'listing_version')
                AND body->>'state' = 'held_off_market'
                AND public.c4_valid_json_uuid(body->'hold_id');
        WHEN 'season_notice' THEN
            RETURN status = 202 AND disposition = 'notice_queued'
                AND public.c4_valid_json_uuid(body->'position_id')
                AND public.c4_valid_json_uuid(body->'seller_user_id')
                AND public.c4_valid_json_uuid(body->'work_id');
        WHEN 'season_close' THEN
            RETURN status = 200 AND disposition = 'season_closed'
                AND jsonb_typeof(body->'target') = 'object'
                AND jsonb_typeof(body->'closed_listing_ids') = 'array';
        ELSE
            RETURN false;
    END CASE;
END
$function$;

-- 既有对象的复合唯一键只为新表引用服务，不改变旧列、旧约束或旧写入语义。
ALTER TABLE public.consignment_listings
    ADD CONSTRAINT c4_listing_identity_uq
        UNIQUE (id, seller_user_id, world_id, external_equipment_id);

ALTER TABLE public.consignment_events
    ADD CONSTRAINT c4_event_command_listing_version_uq
        UNIQUE (command_id, listing_id, state_version);

ALTER TABLE public.inbound_mail
    ADD CONSTRAINT c4_inbound_mail_identity_uq
        UNIQUE (id, world_id, external_mail_id);

CREATE TABLE public.consignment_season_windows (
    id uuid NOT NULL CONSTRAINT c4_season_pk PRIMARY KEY,
    season_id text NOT NULL,
    world_id text NOT NULL CONSTRAINT c4_season_world_fk
        REFERENCES public.worlds(id) ON DELETE RESTRICT,
    timezone text NOT NULL,
    start_utc timestamptz NOT NULL,
    end_utc timestamptz NOT NULL,
    admission_cutoff_utc timestamptz NOT NULL,
    calendar_version bytea NOT NULL,
    preclose_days smallint NOT NULL,
    CONSTRAINT c4_season_id_nonzero CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c4_season_world_ck CHECK (world_id = 'isekai'),
    CONSTRAINT c4_season_text_ck CHECK (
        public.consignment_valid_text(season_id, 128)
        AND public.consignment_valid_text(timezone, 64)
    ),
    CONSTRAINT c4_season_time_ck CHECK (
        isfinite(start_utc) AND isfinite(end_utc) AND isfinite(admission_cutoff_utc)
        AND start_utc < end_utc AND admission_cutoff_utc < end_utc
    ),
    CONSTRAINT c4_season_digest_ck CHECK (public.c4_valid_digest(calendar_version)),
    CONSTRAINT c4_season_preclose_ck CHECK (preclose_days BETWEEN 1 AND 90),
    -- 不变量 10：同一世界的同一季只冻结一次，配置变化从下一季生效。
    CONSTRAINT c4_season_world_season_uq UNIQUE (world_id, season_id),
    CONSTRAINT c4_season_id_world_uq UNIQUE (id, world_id),
    CONSTRAINT c4_season_id_season_uq UNIQUE (id, season_id)
);

CREATE TABLE public.mail_evidence_sessions (
    id uuid NOT NULL CONSTRAINT c4_session_pk PRIMARY KEY,
    bot_uid text NOT NULL,
    world_id text NOT NULL CONSTRAINT c4_session_world_fk
        REFERENCES public.worlds(id) ON DELETE RESTRICT,
    player_name text NOT NULL,
    credential_id uuid NOT NULL CONSTRAINT c4_session_credential_fk
        REFERENCES public.credential_keys(id) ON DELETE RESTRICT,
    credential_version bigint NOT NULL,
    runtime_generation bigint NOT NULL,
    observed_at timestamptz NOT NULL,
    protocol_version text NOT NULL,
    binding_digest bytea NOT NULL,
    CONSTRAINT c4_session_id_nonzero CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c4_session_credential_nonzero CHECK (public.c4_valid_uuid(credential_id)),
    CONSTRAINT c4_session_bot_uid_ck CHECK (bot_uid COLLATE "C" ~ '^[1-9][0-9]{0,31}$'),
    CONSTRAINT c4_session_world_ck CHECK (world_id IN ('main', 'isekai')),
    CONSTRAINT c4_session_text_ck CHECK (
        public.consignment_valid_text(player_name, 256)
        AND public.consignment_valid_text(protocol_version, 64)
    ),
    CONSTRAINT c4_session_versions_ck CHECK (
        credential_version BETWEEN 1 AND 9007199254740991
        AND runtime_generation BETWEEN 1 AND 9007199254740991
    ),
    CONSTRAINT c4_session_observed_ck CHECK (isfinite(observed_at)),
    CONSTRAINT c4_session_digest_ck CHECK (public.c4_valid_digest(binding_digest)),
    CONSTRAINT c4_session_identity_uq UNIQUE (id, bot_uid, world_id),
    CONSTRAINT c4_session_world_uq UNIQUE (id, world_id),
    CONSTRAINT c4_session_binding_uq UNIQUE (binding_digest)
);
CREATE INDEX c4_session_lookup_idx
    ON public.mail_evidence_sessions (bot_uid, world_id, observed_at, id);

CREATE OR REPLACE FUNCTION public.c4_guard_session_credential() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    key_kind text;
    key_scope text;
    key_status text;
    key_version bigint;
BEGIN
    SELECT credential_kind, scope, status, state_version
    INTO key_kind, key_scope, key_status, key_version
    FROM public.credential_keys WHERE id = NEW.credential_id;
    IF key_kind IS DISTINCT FROM 'mail_cookie'
       OR key_scope IS DISTINCT FROM NEW.world_id
       OR key_status IS DISTINCT FROM 'active'
       OR key_version IS DISTINCT FROM NEW.credential_version THEN
        RAISE EXCEPTION '邮件证据会话必须绑定当前 active 的 mail_cookie 凭据'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_session_credential';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c4_session_credential
BEFORE INSERT ON public.mail_evidence_sessions
FOR EACH ROW EXECUTE FUNCTION public.c4_guard_session_credential();

CREATE TABLE public.consignment_mail_readiness (
    world_id text NOT NULL CONSTRAINT c4_readiness_pk PRIMARY KEY
        CONSTRAINT c4_readiness_world_fk
        REFERENCES public.worlds(id) ON DELETE RESTRICT,
    session_id uuid NOT NULL,
    service_epoch bigint NOT NULL,
    policy_digest bytea NOT NULL,
    allowlist_mode text NOT NULL,
    allowed_senders text[] NOT NULL,
    validator_versions text[] NOT NULL,
    detail_ready boolean NOT NULL,
    take_ready boolean NOT NULL,
    return_ready boolean NOT NULL,
    checked_at timestamptz NOT NULL,
    valid_until timestamptz NOT NULL,
    version bigint NOT NULL,
    CONSTRAINT c4_readiness_world_ck CHECK (world_id IN ('main', 'isekai')),
    CONSTRAINT c4_readiness_session_fk FOREIGN KEY (session_id, world_id)
        REFERENCES public.mail_evidence_sessions (id, world_id) ON DELETE RESTRICT,
    CONSTRAINT c4_readiness_mode_ck CHECK (allowlist_mode IN ('enforced', 'disabled')),
    CONSTRAINT c4_readiness_senders_ck CHECK (
        public.c4_valid_text_array(allowed_senders, 1000, 128)
    ),
    CONSTRAINT c4_readiness_validators_ck CHECK (
        public.c4_valid_text_array(validator_versions, 32, 64)
    ),
    CONSTRAINT c4_readiness_epoch_ck CHECK (service_epoch BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c4_readiness_version_ck CHECK (version BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c4_readiness_digest_ck CHECK (public.c4_valid_digest(policy_digest)),
    CONSTRAINT c4_readiness_time_ck CHECK (
        isfinite(checked_at) AND isfinite(valid_until) AND valid_until > checked_at
    )
);

CREATE TABLE public.mail_originals (
    bot_uid text NOT NULL,
    world_id text NOT NULL CONSTRAINT c4_original_world_fk
        REFERENCES public.worlds(id) ON DELETE RESTRICT,
    external_mid text NOT NULL,
    first_session_id uuid NOT NULL,
    received_minute_raw text NOT NULL,
    received_timezone text NOT NULL,
    first_seen_at timestamptz NOT NULL,
    season_window_id uuid,
    CONSTRAINT c4_original_pk PRIMARY KEY (bot_uid, world_id, external_mid),
    CONSTRAINT c4_original_session_fk FOREIGN KEY (first_session_id, bot_uid, world_id)
        REFERENCES public.mail_evidence_sessions (id, bot_uid, world_id) ON DELETE RESTRICT,
    CONSTRAINT c4_original_season_fk FOREIGN KEY (season_window_id, world_id)
        REFERENCES public.consignment_season_windows (id, world_id) ON DELETE RESTRICT,
    CONSTRAINT c4_original_bot_uid_ck CHECK (bot_uid COLLATE "C" ~ '^[1-9][0-9]{0,31}$'),
    CONSTRAINT c4_original_world_ck CHECK (world_id IN ('main', 'isekai')),
    CONSTRAINT c4_original_mid_ck CHECK (external_mid COLLATE "C" ~ '^[1-9][0-9]{0,31}$'),
    CONSTRAINT c4_original_text_ck CHECK (
        public.consignment_valid_text(received_minute_raw, 64)
        AND public.consignment_valid_text(received_timezone, 64)
    ),
    CONSTRAINT c4_original_time_ck CHECK (isfinite(first_seen_at)),
    CONSTRAINT c4_original_season_shape_ck CHECK (
        (world_id = 'main' AND season_window_id IS NULL)
        OR world_id = 'isekai'
    )
);

CREATE TABLE public.mail_evidence_envelopes (
    id uuid NOT NULL CONSTRAINT c4_envelope_pk PRIMARY KEY,
    purpose text NOT NULL,
    aad_owner text NOT NULL,
    ciphertext bytea,
    nonce bytea,
    encryption_key_id text,
    fingerprint text,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    retired_at timestamptz,
    CONSTRAINT c4_envelope_id_nonzero CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c4_envelope_text_ck CHECK (
        public.consignment_valid_text(purpose, 64)
        AND public.consignment_valid_text(aad_owner, 128)
    ),
    CONSTRAINT c4_envelope_times_ck CHECK (
        isfinite(created_at) AND isfinite(updated_at) AND updated_at >= created_at
        AND (retired_at IS NULL OR isfinite(retired_at))
    ),
    CONSTRAINT c4_envelope_shape_ck CHECK ((
        CASE WHEN retired_at IS NULL THEN
            octet_length(ciphertext) > 0 AND octet_length(nonce) = 12
            AND encryption_key_id COLLATE "C" ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}$'
            AND fingerprint COLLATE "C" ~ '^[a-f0-9]{64}$'
        ELSE ciphertext IS NULL AND nonce IS NULL AND encryption_key_id IS NULL
             AND fingerprint IS NULL AND retired_at = updated_at END
    ) IS TRUE)
);
CREATE INDEX c4_envelope_active_key_idx
    ON public.mail_evidence_envelopes (encryption_key_id, id) WHERE retired_at IS NULL;

CREATE TABLE public.mail_envelope_observations (
    id uuid NOT NULL CONSTRAINT c4_observation_pk PRIMARY KEY,
    bot_uid text NOT NULL,
    world_id text NOT NULL CONSTRAINT c4_observation_world_fk
        REFERENCES public.worlds(id) ON DELETE RESTRICT,
    external_mid text NOT NULL,
    remote_observation_id text NOT NULL,
    session_id uuid NOT NULL,
    mailbox_key text NOT NULL,
    quarantine_id uuid,
    quarantine_version bigint,
    observed_at timestamptz NOT NULL,
    parser_version text NOT NULL,
    evidence_ref text,
    subject_digest bytea NOT NULL,
    body_digest bytea NOT NULL,
    manifest_digest bytea NOT NULL,
    sender text,
    recipient text,
    returnable boolean,
    takeable boolean,
    credits bigint,
    cod bigint,
    raw_count integer,
    manifest jsonb NOT NULL,
    derived_facts jsonb NOT NULL,
    season_window_id uuid,
    CONSTRAINT c4_observation_id_nonzero CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c4_observation_bot_uid_ck CHECK (bot_uid COLLATE "C" ~ '^[1-9][0-9]{0,31}$'),
    CONSTRAINT c4_observation_world_ck CHECK (world_id IN ('main', 'isekai')),
    CONSTRAINT c4_observation_mid_ck CHECK (external_mid COLLATE "C" ~ '^[1-9][0-9]{0,31}$'),
    CONSTRAINT c4_observation_session_fk FOREIGN KEY (session_id, bot_uid, world_id)
        REFERENCES public.mail_evidence_sessions (id, bot_uid, world_id) ON DELETE RESTRICT,
    CONSTRAINT c4_observation_season_fk FOREIGN KEY (season_window_id, world_id)
        REFERENCES public.consignment_season_windows (id, world_id) ON DELETE RESTRICT,
    CONSTRAINT c4_observation_text_ck CHECK (
        public.consignment_valid_text(remote_observation_id, 128)
        AND public.consignment_valid_text(mailbox_key, 256)
        AND public.consignment_valid_text(parser_version, 64)
        AND (evidence_ref IS NULL OR public.consignment_safe_observation_text(evidence_ref, 512))
        AND (sender IS NULL OR public.consignment_safe_observation_text(sender, 256))
        AND (recipient IS NULL OR public.consignment_safe_observation_text(recipient, 256))
    ),
    CONSTRAINT c4_observation_time_ck CHECK (isfinite(observed_at)),
    CONSTRAINT c4_observation_digest_ck CHECK (
        public.c4_valid_digest(subject_digest)
        AND public.c4_valid_digest(body_digest)
        AND public.c4_valid_digest(manifest_digest)
    ),
    CONSTRAINT c4_observation_quarantine_ck CHECK (
        ((quarantine_id IS NULL AND quarantine_version IS NULL)
        OR (public.c4_valid_uuid(quarantine_id)
            AND quarantine_version BETWEEN 1 AND 9007199254740991)) IS TRUE
    ),
    CONSTRAINT c4_observation_amount_ck CHECK (
        (credits IS NULL OR credits BETWEEN 0 AND 1000000000)
        AND (cod IS NULL OR cod BETWEEN 0 AND 1000000000)
        AND (raw_count IS NULL OR raw_count BETWEEN 1 AND 200)
    ),
    CONSTRAINT c4_observation_manifest_ck CHECK (public.c4_valid_manifest(manifest)),
    CONSTRAINT c4_observation_derived_ck CHECK (public.c4_valid_derived_facts(derived_facts)),
    CONSTRAINT c4_observation_season_shape_ck CHECK (
        (world_id = 'main' AND season_window_id IS NULL) OR world_id = 'isekai'
    ),
    CONSTRAINT c4_observation_remote_uq UNIQUE (bot_uid, world_id, external_mid, remote_observation_id),
    CONSTRAINT c4_observation_id_uq UNIQUE (bot_uid, world_id, external_mid, id),
    CONSTRAINT c4_observation_quarantine_id_uq
        UNIQUE (quarantine_id, bot_uid, world_id, external_mid, id)
);
CREATE INDEX c4_observation_quarantine_idx
    ON public.mail_envelope_observations (quarantine_id, quarantine_version);

CREATE TABLE public.mail_original_links (
    inbound_id uuid NOT NULL CONSTRAINT c4_link_pk PRIMARY KEY
        CONSTRAINT c4_link_inbound_fk
        REFERENCES public.inbound_mail(id) ON DELETE RESTRICT,
    mailbox_key text NOT NULL,
    external_mid text NOT NULL,
    bot_uid text NOT NULL,
    world_id text NOT NULL CONSTRAINT c4_link_world_fk
        REFERENCES public.worlds(id) ON DELETE RESTRICT,
    auth_observation_id uuid NOT NULL,
    CONSTRAINT c4_link_key_fk FOREIGN KEY (bot_uid, world_id, external_mid)
        REFERENCES public.mail_originals (bot_uid, world_id, external_mid)
        ON DELETE RESTRICT,
    CONSTRAINT c4_link_observation_fk
        FOREIGN KEY (bot_uid, world_id, external_mid, auth_observation_id)
        REFERENCES public.mail_envelope_observations (bot_uid, world_id, external_mid, id)
        ON DELETE RESTRICT,
    CONSTRAINT c4_link_inbound_identity_fk
        FOREIGN KEY (inbound_id, world_id, external_mid)
        REFERENCES public.inbound_mail (id, world_id, external_mail_id)
        ON DELETE RESTRICT,
    CONSTRAINT c4_link_text_ck CHECK (
        public.consignment_valid_text(mailbox_key, 256)
        AND external_mid COLLATE "C" ~ '^[1-9][0-9]{0,31}$'
        AND bot_uid COLLATE "C" ~ '^[1-9][0-9]{0,31}$'
        AND world_id IN ('main', 'isekai')
    ),
    CONSTRAINT c4_link_mailbox_uq UNIQUE (mailbox_key, external_mid)
);
CREATE INDEX c4_link_key_idx
    ON public.mail_original_links (bot_uid, world_id, external_mid, inbound_id);

CREATE TABLE public.mail_observation_conflicts (
    observation_id uuid NOT NULL CONSTRAINT c4_conflict_observation_fk
        REFERENCES public.mail_envelope_observations(id) ON DELETE RESTRICT,
    conflicting_digest bytea NOT NULL,
    evidence_ref text,
    observed_at timestamptz NOT NULL,
    CONSTRAINT c4_conflict_pk PRIMARY KEY (observation_id, conflicting_digest),
    CONSTRAINT c4_conflict_digest_ck CHECK (public.c4_valid_digest(conflicting_digest)),
    CONSTRAINT c4_conflict_evidence_ck CHECK (
        evidence_ref IS NULL OR public.consignment_safe_observation_text(evidence_ref, 512)
    ),
    CONSTRAINT c4_conflict_time_ck CHECK (isfinite(observed_at))
);

CREATE TABLE public.mail_initial_decisions (
    bot_uid text NOT NULL,
    world_id text NOT NULL CONSTRAINT c4_initial_world_fk
        REFERENCES public.worlds(id) ON DELETE RESTRICT,
    external_mid text NOT NULL,
    id uuid NOT NULL,
    observation_id uuid NOT NULL,
    command_id uuid NOT NULL CONSTRAINT c4_initial_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    kind text NOT NULL,
    quarantine_id uuid,
    decided_at timestamptz NOT NULL,
    CONSTRAINT c4_initial_decision_pk PRIMARY KEY (bot_uid, world_id, external_mid),
    CONSTRAINT c4_initial_decision_observation_fk
        FOREIGN KEY (bot_uid, world_id, external_mid, observation_id)
        REFERENCES public.mail_envelope_observations (bot_uid, world_id, external_mid, id)
        ON DELETE RESTRICT,
    CONSTRAINT c4_initial_decision_kind_ck CHECK (
        kind IN ('accepted', 'return_decided', 'archived_empty', 'quarantined')
    ),
    CONSTRAINT c4_initial_decision_quarantine_ck CHECK (
        (kind = 'quarantined') = (quarantine_id IS NOT NULL)
    ),
    CONSTRAINT c4_initial_decision_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c4_initial_decision_time_ck CHECK (isfinite(decided_at)),
    CONSTRAINT c4_initial_decision_id_uq UNIQUE (id),
    CONSTRAINT c4_initial_decision_key_id_uq UNIQUE (bot_uid, world_id, external_mid, id),
    CONSTRAINT c4_initial_decision_key_id_kind_uq
        UNIQUE (bot_uid, world_id, external_mid, id, kind)
);

CREATE TABLE public.mail_quarantines (
    id uuid NOT NULL CONSTRAINT c4_quarantine_pk PRIMARY KEY,
    bot_uid text NOT NULL,
    world_id text NOT NULL CONSTRAINT c4_quarantine_world_fk
        REFERENCES public.worlds(id) ON DELETE RESTRICT,
    external_mid text NOT NULL,
    initial_decision_id uuid NOT NULL,
    version bigint NOT NULL,
    state text NOT NULL,
    reread_locator text NOT NULL,
    responsibility_code text NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT c4_quarantine_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c4_quarantine_key_uq UNIQUE (bot_uid, world_id, external_mid),
    CONSTRAINT c4_quarantine_decision_uq UNIQUE (initial_decision_id),
    CONSTRAINT c4_quarantine_initial_fk
        FOREIGN KEY (bot_uid, world_id, external_mid, initial_decision_id)
        REFERENCES public.mail_initial_decisions (bot_uid, world_id, external_mid, id)
        ON DELETE RESTRICT,
    CONSTRAINT c4_quarantine_state_ck CHECK (state IN ('active', 'released')),
    CONSTRAINT c4_quarantine_version_ck CHECK (version BETWEEN 1 AND 9007199254740991),
    -- responsibility_code 存的就是 4A `QuarantineReason` 的隔离原因闭集。
    CONSTRAINT c4_quarantine_responsibility_ck CHECK (responsibility_code IN (
        'duplicate_raw_eid', 'canonical_alias_ambiguous', 'unknown_attachment',
        'list_detail_conflict', 'asset_count_unknown', 'same_name_changed_uid',
        'identity_unproven', 'amount_unproven', 'return_unproven', 'take_unproven',
        'unsafe_return_shape', 'suspected_system_rts'
    )),
    CONSTRAINT c4_quarantine_text_ck CHECK (
        public.consignment_safe_observation_text(reread_locator, 512)
        AND public.consignment_valid_text(responsibility_code, 64)
    ),
    CONSTRAINT c4_quarantine_time_ck CHECK (
        isfinite(created_at) AND isfinite(updated_at) AND updated_at >= created_at
    ),
    CONSTRAINT c4_quarantine_key_id_uq UNIQUE (bot_uid, world_id, external_mid, id)
);
CREATE INDEX c4_quarantine_state_idx ON public.mail_quarantines (state, updated_at, id);

CREATE TABLE public.mail_quarantine_rechecks (
    id uuid NOT NULL CONSTRAINT c4_recheck_pk PRIMARY KEY,
    quarantine_id uuid NOT NULL CONSTRAINT c4_recheck_quarantine_fk
        REFERENCES public.mail_quarantines(id) ON DELETE RESTRICT,
    requested_version bigint NOT NULL,
    command_id uuid NOT NULL CONSTRAINT c4_recheck_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    state text NOT NULL,
    available_at timestamptz NOT NULL,
    lease_token uuid,
    lease_until timestamptz,
    fence bigint NOT NULL,
    attempt_count integer NOT NULL,
    completed_at timestamptz,
    CONSTRAINT c4_recheck_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c4_recheck_version_ck CHECK (requested_version BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c4_recheck_state_ck CHECK (state IN ('queued', 'leased', 'done', 'cancelled')),
    CONSTRAINT c4_recheck_time_ck CHECK (
        isfinite(available_at) AND (lease_until IS NULL OR isfinite(lease_until))
        AND (completed_at IS NULL OR isfinite(completed_at))
    ),
    CONSTRAINT c4_recheck_shape_ck CHECK ((
        CASE state
            WHEN 'queued' THEN lease_token IS NULL AND lease_until IS NULL AND completed_at IS NULL
            WHEN 'leased' THEN public.c4_valid_uuid(lease_token) AND lease_until IS NOT NULL
                AND fence > 0 AND attempt_count > 0 AND completed_at IS NULL
            WHEN 'done' THEN lease_token IS NULL AND lease_until IS NULL AND completed_at IS NOT NULL
            WHEN 'cancelled' THEN lease_token IS NULL AND lease_until IS NULL
                AND completed_at IS NOT NULL
        END
    ) IS TRUE),
    CONSTRAINT c4_recheck_counts_ck CHECK (fence >= 0 AND attempt_count >= 0),
    CONSTRAINT c4_recheck_command_uq UNIQUE (command_id)
);
CREATE UNIQUE INDEX c4_recheck_active_uq
    ON public.mail_quarantine_rechecks (quarantine_id, requested_version)
    WHERE state IN ('queued', 'leased');
CREATE INDEX c4_recheck_queue_idx
    ON public.mail_quarantine_rechecks (state, available_at, id);

CREATE TABLE public.mail_final_dispositions (
    bot_uid text NOT NULL,
    world_id text NOT NULL CONSTRAINT c4_final_world_fk
        REFERENCES public.worlds(id) ON DELETE RESTRICT,
    external_mid text NOT NULL,
    id uuid NOT NULL,
    kind text NOT NULL,
    source_observation_id uuid NOT NULL,
    command_id uuid NOT NULL CONSTRAINT c4_final_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    decided_at timestamptz NOT NULL,
    CONSTRAINT c4_final_pk PRIMARY KEY (bot_uid, world_id, external_mid),
    CONSTRAINT c4_final_observation_fk
        FOREIGN KEY (bot_uid, world_id, external_mid, source_observation_id)
        REFERENCES public.mail_envelope_observations (bot_uid, world_id, external_mid, id)
        ON DELETE RESTRICT,
    CONSTRAINT c4_final_kind_ck CHECK (kind IN (
        'accepted', 'return_decided', 'archived_empty', 'acquire_return',
        'auction_return_case', 'auction_recovery_return'
    )),
    CONSTRAINT c4_final_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c4_final_time_ck CHECK (isfinite(decided_at)),
    CONSTRAINT c4_final_id_uq UNIQUE (id),
    CONSTRAINT c4_final_key_id_uq UNIQUE (bot_uid, world_id, external_mid, id),
    CONSTRAINT c4_final_key_id_kind_uq UNIQUE (bot_uid, world_id, external_mid, id, kind)
);

CREATE TABLE public.settlement_recovery_return_handoffs (
    id uuid NOT NULL CONSTRAINT c4_handoff_pk PRIMARY KEY,
    bot_uid text NOT NULL,
    world_id text NOT NULL CONSTRAINT c4_handoff_world_fk
        REFERENCES public.worlds(id) ON DELETE RESTRICT,
    external_mid text NOT NULL,
    return_mail_id uuid NOT NULL CONSTRAINT c4_handoff_mail_fk
        REFERENCES public.inbound_mail(id) ON DELETE RESTRICT,
    recovery_id uuid NOT NULL CONSTRAINT c4_handoff_recovery_fk
        REFERENCES public.settlement_recoveries(id) ON DELETE RESTRICT,
    obligation_id uuid NOT NULL CONSTRAINT c4_handoff_obligation_fk
        REFERENCES public.settlement_obligations(id) ON DELETE RESTRICT,
    observation_id uuid NOT NULL,
    accepted_at timestamptz NOT NULL,
    CONSTRAINT c4_handoff_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c4_handoff_key_uq UNIQUE (bot_uid, world_id, external_mid),
    CONSTRAINT c4_handoff_mail_uq UNIQUE (return_mail_id),
    CONSTRAINT c4_handoff_observation_fk
        FOREIGN KEY (bot_uid, world_id, external_mid, observation_id)
        REFERENCES public.mail_envelope_observations (bot_uid, world_id, external_mid, id)
        ON DELETE RESTRICT,
    CONSTRAINT c4_handoff_mail_identity_fk
        FOREIGN KEY (return_mail_id, world_id, external_mid)
        REFERENCES public.inbound_mail (id, world_id, external_mail_id)
        ON DELETE RESTRICT,
    CONSTRAINT c4_handoff_key_id_uq UNIQUE (bot_uid, world_id, external_mid, id),
    CONSTRAINT c4_handoff_time_ck CHECK (isfinite(accepted_at))
);
CREATE INDEX c4_handoff_recovery_idx
    ON public.settlement_recovery_return_handoffs (recovery_id, accepted_at, id);

CREATE TABLE public.mail_quarantine_releases (
    id uuid NOT NULL CONSTRAINT c4_release_pk PRIMARY KEY,
    bot_uid text NOT NULL,
    world_id text NOT NULL CONSTRAINT c4_release_world_fk
        REFERENCES public.worlds(id) ON DELETE RESTRICT,
    external_mid text NOT NULL,
    quarantine_id uuid NOT NULL,
    expected_version bigint NOT NULL,
    observation_id uuid NOT NULL,
    decision_id uuid NOT NULL,
    kind text NOT NULL,
    command_id uuid NOT NULL CONSTRAINT c4_release_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    case_return_mail_id uuid CONSTRAINT c4_release_case_mail_fk
        REFERENCES public.settlement_return_cases(return_mail_id) ON DELETE RESTRICT,
    case_evidence_id uuid CONSTRAINT c4_release_case_evidence_fk
        REFERENCES public.settlement_return_evidence(return_mail_id) ON DELETE RESTRICT,
    recovery_handoff_id uuid,
    released_at timestamptz NOT NULL,
    CONSTRAINT c4_release_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c4_release_quarantine_uq UNIQUE (quarantine_id),
    CONSTRAINT c4_release_key_uq UNIQUE (bot_uid, world_id, external_mid),
    CONSTRAINT c4_release_decision_uq UNIQUE (decision_id),
    CONSTRAINT c4_release_command_uq UNIQUE (command_id),
    CONSTRAINT c4_release_quarantine_fk
        FOREIGN KEY (quarantine_id, bot_uid, world_id, external_mid, observation_id)
        REFERENCES public.mail_envelope_observations
            (quarantine_id, bot_uid, world_id, external_mid, id)
        ON DELETE RESTRICT,
    CONSTRAINT c4_release_decision_fk
        FOREIGN KEY (bot_uid, world_id, external_mid, decision_id, kind)
        REFERENCES public.mail_final_dispositions (bot_uid, world_id, external_mid, id, kind)
        ON DELETE RESTRICT,
    CONSTRAINT c4_release_handoff_fk
        FOREIGN KEY (bot_uid, world_id, external_mid, recovery_handoff_id)
        REFERENCES public.settlement_recovery_return_handoffs (bot_uid, world_id, external_mid, id)
        ON DELETE RESTRICT,
    CONSTRAINT c4_release_version_ck CHECK (expected_version BETWEEN 1 AND 9007199254740991),
    -- 行内 kind 与终局 kind 同名，供 (K,decision_id,kind) 复合 FK；wire 的 decision_kind
    -- 仍使用 accept/return/archive_empty，由应用层映射。
    CONSTRAINT c4_release_kind_ck CHECK (kind IN (
        'accepted', 'return_decided', 'archived_empty', 'acquire_return',
        'auction_return_case', 'auction_recovery_return'
    )),
    CONSTRAINT c4_release_branch_ck CHECK ((
        CASE kind
            WHEN 'auction_return_case' THEN
                case_return_mail_id IS NOT NULL AND case_evidence_id = case_return_mail_id
                AND recovery_handoff_id IS NULL
            WHEN 'auction_recovery_return' THEN
                recovery_handoff_id IS NOT NULL
                AND case_return_mail_id IS NULL AND case_evidence_id IS NULL
            ELSE case_return_mail_id IS NULL AND case_evidence_id IS NULL
                AND recovery_handoff_id IS NULL
        END
    ) IS TRUE),
    CONSTRAINT c4_release_time_ck CHECK (isfinite(released_at))
);

CREATE TABLE public.consignment_deposit_reservations (
    id uuid NOT NULL CONSTRAINT c4_res_pk PRIMARY KEY,
    listing_id uuid NOT NULL,
    seller_user_id uuid NOT NULL CONSTRAINT c4_res_seller_fk
        REFERENCES public.users(id) ON DELETE RESTRICT,
    verified_sender text NOT NULL,
    bot_uid text NOT NULL,
    world_id text NOT NULL CONSTRAINT c4_res_world_fk
        REFERENCES public.worlds(id) ON DELETE RESTRICT,
    canonical_eid text NOT NULL,
    season_window_id uuid,
    generation bigint NOT NULL,
    service_epoch bigint NOT NULL,
    session_id uuid NOT NULL,
    token_digest bytea NOT NULL,
    issued_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    state text NOT NULL,
    last_command_id uuid NOT NULL CONSTRAINT c4_res_last_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    retired_at timestamptz,
    reason text,
    CONSTRAINT c4_res_listing_fk
        FOREIGN KEY (listing_id, seller_user_id, world_id, canonical_eid)
        REFERENCES public.consignment_listings
            (id, seller_user_id, world_id, external_equipment_id)
        ON DELETE RESTRICT,
    CONSTRAINT c4_res_session_fk FOREIGN KEY (session_id, bot_uid, world_id)
        REFERENCES public.mail_evidence_sessions (id, bot_uid, world_id) ON DELETE RESTRICT,
    CONSTRAINT c4_res_season_fk FOREIGN KEY (season_window_id, world_id)
        REFERENCES public.consignment_season_windows (id, world_id) ON DELETE RESTRICT,
    CONSTRAINT c4_res_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c4_res_world_ck CHECK (world_id IN ('main', 'isekai')),
    CONSTRAINT c4_res_bot_uid_ck CHECK (bot_uid COLLATE "C" ~ '^[1-9][0-9]{0,31}$'),
    CONSTRAINT c4_res_eid_ck CHECK (canonical_eid COLLATE "C" ~ '^[1-9][0-9]{0,31}$'),
    CONSTRAINT c4_res_text_ck CHECK (public.consignment_valid_text(verified_sender, 256)),
    CONSTRAINT c4_res_versions_ck CHECK (
        generation BETWEEN 1 AND 9007199254740991
        AND service_epoch BETWEEN 1 AND 9007199254740991
    ),
    CONSTRAINT c4_res_digest_ck CHECK (public.c4_valid_digest(token_digest)),
    CONSTRAINT c4_res_time_ck CHECK (
        isfinite(issued_at) AND isfinite(expires_at) AND expires_at > issued_at
        AND (retired_at IS NULL OR isfinite(retired_at))
    ),
    CONSTRAINT c4_res_state_ck CHECK ((
        CASE state
            WHEN 'active' THEN retired_at IS NULL AND reason IS NULL
            WHEN 'retired' THEN retired_at IS NOT NULL
                AND reason IN ('expired', 'cancelled', 'identity_lost', 'service_disabled', 'reissued')
            WHEN 'consumed' THEN retired_at IS NULL AND reason IS NULL
        END
    ) IS TRUE),
    CONSTRAINT c4_res_season_shape_ck CHECK (
        (world_id = 'main' AND season_window_id IS NULL) OR world_id = 'isekai'
    ),
    CONSTRAINT c4_res_token_uq UNIQUE (token_digest),
    CONSTRAINT c4_res_generation_uq UNIQUE (listing_id, generation),
    CONSTRAINT c4_res_id_generation_seller_uq UNIQUE (id, generation, seller_user_id),
    CONSTRAINT c4_res_id_world_eid_uq UNIQUE (id, world_id, canonical_eid),
    CONSTRAINT c4_res_id_world_eid_seller_uq UNIQUE (id, world_id, canonical_eid, seller_user_id)
);
CREATE UNIQUE INDEX c4_res_one_active_idx
    ON public.consignment_deposit_reservations (listing_id) WHERE state = 'active';
CREATE INDEX c4_res_expiry_idx
    ON public.consignment_deposit_reservations (state, expires_at, id);

CREATE TABLE public.consignment_reservation_secrets (
    reservation_id uuid NOT NULL CONSTRAINT c4_res_secret_pk PRIMARY KEY
        CONSTRAINT c4_res_secret_reservation_fk
        REFERENCES public.consignment_deposit_reservations(id) ON DELETE RESTRICT,
    generation bigint NOT NULL,
    seller_user_id uuid NOT NULL,
    ciphertext bytea,
    nonce bytea,
    encryption_key_id text,
    fingerprint text,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    retired_at timestamptz,
    CONSTRAINT c4_res_secret_identity_fk
        FOREIGN KEY (reservation_id, generation, seller_user_id)
        REFERENCES public.consignment_deposit_reservations (id, generation, seller_user_id)
        ON DELETE RESTRICT,
    CONSTRAINT c4_res_secret_generation_ck CHECK (generation BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c4_res_secret_time_ck CHECK (
        isfinite(created_at) AND isfinite(updated_at) AND updated_at >= created_at
        AND (retired_at IS NULL OR isfinite(retired_at))
    ),
    CONSTRAINT c4_res_secret_shape_ck CHECK ((
        CASE WHEN retired_at IS NULL THEN
            octet_length(ciphertext) > 0 AND octet_length(nonce) = 12
            AND encryption_key_id COLLATE "C" ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}$'
            AND fingerprint COLLATE "C" ~ '^[a-f0-9]{64}$'
        ELSE ciphertext IS NULL AND nonce IS NULL AND encryption_key_id IS NULL
             AND fingerprint IS NULL AND retired_at = updated_at END
    ) IS TRUE)
);
CREATE INDEX c4_res_secret_active_key_idx
    ON public.consignment_reservation_secrets (encryption_key_id, reservation_id)
    WHERE retired_at IS NULL;

CREATE TABLE public.consignment_custody_assets (
    equipment_id uuid NOT NULL CONSTRAINT c4_asset_pk PRIMARY KEY,
    world_id text NOT NULL CONSTRAINT c4_asset_world_fk
        REFERENCES public.worlds(id) ON DELETE RESTRICT,
    canonical_eid text NOT NULL,
    season_id text,
    first_observation_id uuid NOT NULL CONSTRAINT c4_asset_observation_fk
        REFERENCES public.mail_envelope_observations(id) ON DELETE RESTRICT,
    auction_equipment_id uuid CONSTRAINT c4_asset_auction_equipment_fk
        REFERENCES public.equipment(id) ON DELETE RESTRICT,
    CONSTRAINT c4_asset_id_ck CHECK (public.c4_valid_uuid(equipment_id)),
    CONSTRAINT c4_asset_world_ck CHECK (world_id IN ('main', 'isekai')),
    CONSTRAINT c4_asset_eid_ck CHECK (canonical_eid COLLATE "C" ~ '^[1-9][0-9]{0,31}$'),
    CONSTRAINT c4_asset_season_shape_ck CHECK (
        (world_id = 'main' AND season_id IS NULL)
        OR (world_id = 'isekai' AND (season_id IS NULL OR public.consignment_valid_text(season_id, 128)))
    ),
    CONSTRAINT c4_asset_identity_uq UNIQUE NULLS NOT DISTINCT
        (world_id, canonical_eid, season_id),
    CONSTRAINT c4_asset_identity_composite_uq UNIQUE NULLS NOT DISTINCT
        (equipment_id, world_id, canonical_eid, season_id),
    CONSTRAINT c4_asset_equipment_identity_uq UNIQUE (equipment_id, world_id, canonical_eid)
);

CREATE TABLE public.consignment_equipment_claims (
    world_id text NOT NULL CONSTRAINT c4_claim_world_fk
        REFERENCES public.worlds(id) ON DELETE RESTRICT,
    canonical_eid text NOT NULL,
    revision bigint NOT NULL,
    reservation_id uuid CONSTRAINT c4_claim_reservation_fk
        REFERENCES public.consignment_deposit_reservations(id) ON DELETE RESTRICT,
    position_id uuid,
    changed_at timestamptz NOT NULL,
    last_fact_id uuid NOT NULL,
    CONSTRAINT c4_claim_pk PRIMARY KEY (world_id, canonical_eid),
    CONSTRAINT c4_claim_world_ck CHECK (world_id IN ('main', 'isekai')),
    -- 旧拍卖装备编号可以有更长的历史实数串；实物互斥行必须容纳它们，不能因长度拒绝旧 lot。
    CONSTRAINT c4_claim_eid_ck CHECK (canonical_eid COLLATE "C" ~ '^[1-9][0-9]*$'),
    CONSTRAINT c4_claim_revision_ck CHECK (revision BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c4_claim_time_ck CHECK (isfinite(changed_at)),
    CONSTRAINT c4_claim_fact_ck CHECK (public.c4_valid_uuid(last_fact_id)),
    CONSTRAINT c4_claim_holder_ck CHECK (
        reservation_id IS NULL OR position_id IS NULL
    )
);
CREATE UNIQUE INDEX c4_claim_reservation_uq
    ON public.consignment_equipment_claims (reservation_id) WHERE reservation_id IS NOT NULL;
CREATE UNIQUE INDEX c4_claim_position_uq
    ON public.consignment_equipment_claims (position_id) WHERE position_id IS NOT NULL;

CREATE TABLE public.consignment_deposit_items (
    id uuid NOT NULL CONSTRAINT c4_item_pk PRIMARY KEY,
    bot_uid text NOT NULL,
    world_id text NOT NULL CONSTRAINT c4_item_world_fk
        REFERENCES public.worlds(id) ON DELETE RESTRICT,
    external_mid text NOT NULL,
    decision_id uuid NOT NULL,
    ordinal integer NOT NULL,
    canonical_eid text NOT NULL,
    reservation_id uuid NOT NULL,
    listing_id uuid NOT NULL,
    seller_user_id uuid NOT NULL CONSTRAINT c4_item_seller_fk
        REFERENCES public.users(id) ON DELETE RESTRICT,
    equipment_id uuid NOT NULL CONSTRAINT c4_item_asset_equipment_fk
        REFERENCES public.consignment_custody_assets(equipment_id) ON DELETE RESTRICT,
    accepted_listing_version bigint NOT NULL,
    accepted_price bigint NOT NULL,
    credential_ref uuid NOT NULL CONSTRAINT c4_item_credential_fk
        REFERENCES public.mail_evidence_envelopes(id) ON DELETE RESTRICT,
    observation_id uuid NOT NULL,
    CONSTRAINT c4_item_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c4_item_observation_fk
        FOREIGN KEY (bot_uid, world_id, external_mid, observation_id)
        REFERENCES public.mail_envelope_observations (bot_uid, world_id, external_mid, id)
        ON DELETE RESTRICT,
    CONSTRAINT c4_item_decision_fk
        FOREIGN KEY (bot_uid, world_id, external_mid, decision_id)
        REFERENCES public.mail_final_dispositions (bot_uid, world_id, external_mid, id)
        ON DELETE RESTRICT,
    CONSTRAINT c4_item_reservation_fk
        FOREIGN KEY (reservation_id, world_id, canonical_eid, seller_user_id)
        REFERENCES public.consignment_deposit_reservations
            (id, world_id, canonical_eid, seller_user_id)
        ON DELETE RESTRICT,
    CONSTRAINT c4_item_listing_fk
        FOREIGN KEY (listing_id, seller_user_id, world_id, canonical_eid)
        REFERENCES public.consignment_listings
            (id, seller_user_id, world_id, external_equipment_id)
        ON DELETE RESTRICT,
    CONSTRAINT c4_item_asset_fk
        FOREIGN KEY (equipment_id, world_id, canonical_eid)
        REFERENCES public.consignment_custody_assets (equipment_id, world_id, canonical_eid)
        ON DELETE RESTRICT,
    CONSTRAINT c4_item_eid_ck CHECK (canonical_eid COLLATE "C" ~ '^[1-9][0-9]{0,31}$'),
    CONSTRAINT c4_item_ordinal_ck CHECK (ordinal BETWEEN 1 AND 20),
    CONSTRAINT c4_item_versions_ck CHECK (
        accepted_listing_version BETWEEN 1 AND 9007199254740991
        AND accepted_price BETWEEN 1 AND 1000000000
    ),
    CONSTRAINT c4_item_ordinal_uq UNIQUE (decision_id, ordinal),
    CONSTRAINT c4_item_eid_uq UNIQUE (decision_id, canonical_eid),
    CONSTRAINT c4_item_reservation_uq UNIQUE (reservation_id),
    CONSTRAINT c4_item_position_identity_uq
        UNIQUE (id, listing_id, seller_user_id, equipment_id, world_id, canonical_eid)
);
CREATE INDEX c4_item_listing_idx ON public.consignment_deposit_items (listing_id);

CREATE TABLE public.consignment_custody_positions (
    id uuid NOT NULL CONSTRAINT c4_position_pk PRIMARY KEY,
    item_id uuid NOT NULL,
    listing_id uuid NOT NULL,
    seller_user_id uuid NOT NULL,
    equipment_id uuid NOT NULL,
    world_id text NOT NULL,
    canonical_eid text NOT NULL,
    season_window_id uuid,
    state text NOT NULL,
    version bigint NOT NULL,
    current_generation bigint,
    updated_at timestamptz NOT NULL,
    CONSTRAINT c4_position_item_fk
        FOREIGN KEY (item_id, listing_id, seller_user_id, equipment_id, world_id, canonical_eid)
        REFERENCES public.consignment_deposit_items
            (id, listing_id, seller_user_id, equipment_id, world_id, canonical_eid)
        ON DELETE RESTRICT,
    CONSTRAINT c4_position_listing_fk
        FOREIGN KEY (listing_id, seller_user_id, world_id, canonical_eid)
        REFERENCES public.consignment_listings
            (id, seller_user_id, world_id, external_equipment_id)
        ON DELETE RESTRICT,
    CONSTRAINT c4_position_asset_fk FOREIGN KEY (equipment_id, world_id, canonical_eid)
        REFERENCES public.consignment_custody_assets (equipment_id, world_id, canonical_eid)
        ON DELETE RESTRICT,
    CONSTRAINT c4_position_season_fk FOREIGN KEY (season_window_id, world_id)
        REFERENCES public.consignment_season_windows (id, world_id) ON DELETE RESTRICT,
    CONSTRAINT c4_position_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c4_position_world_ck CHECK (world_id IN ('main', 'isekai')),
    CONSTRAINT c4_position_state_ck CHECK (state IN (
        'awaiting_take', 'in_custody', 'committed', 'in_transit', 'reconcile',
        'handed_over', 'season_ended', 'lost'
    )),
    CONSTRAINT c4_position_version_ck CHECK (version BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c4_position_generation_ck CHECK (
        current_generation IS NULL OR current_generation BETWEEN 1 AND 9007199254740991
    ),
    CONSTRAINT c4_position_time_ck CHECK (isfinite(updated_at)),
    CONSTRAINT c4_position_season_shape_ck CHECK (
        (world_id = 'main' AND season_window_id IS NULL) OR world_id = 'isekai'
    ),
    CONSTRAINT c4_position_item_uq UNIQUE (item_id),
    CONSTRAINT c4_position_id_world_eid_uq UNIQUE (id, world_id, canonical_eid),
    CONSTRAINT c4_position_id_seller_uq UNIQUE (id, seller_user_id),
    CONSTRAINT c4_position_season_id_uq UNIQUE (season_window_id, id)
);
CREATE INDEX c4_position_seller_state_idx
    ON public.consignment_custody_positions (seller_user_id, state, id);
CREATE INDEX c4_position_season_state_idx
    ON public.consignment_custody_positions (season_window_id, state, id);

CREATE TABLE public.consignment_disposition_generations (
    position_id uuid NOT NULL CONSTRAINT c4_generation_position_fk
        REFERENCES public.consignment_custody_positions(id) ON DELETE RESTRICT,
    generation bigint NOT NULL,
    purpose text NOT NULL,
    predecessor bigint,
    proof_id uuid NOT NULL,
    service_epoch bigint NOT NULL,
    blacklist_baseline_ids uuid[] NOT NULL,
    season_window_id uuid,
    created_command_id uuid NOT NULL CONSTRAINT c4_generation_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    created_at timestamptz NOT NULL,
    CONSTRAINT c4_generation_pk PRIMARY KEY (position_id, generation),
    CONSTRAINT c4_generation_season_fk FOREIGN KEY (season_window_id, position_id)
        REFERENCES public.consignment_custody_positions (season_window_id, id)
        ON DELETE RESTRICT,
    CONSTRAINT c4_generation_purpose_ck CHECK (public.consignment_valid_text(purpose, 64)),
    CONSTRAINT c4_generation_number_ck CHECK (
        generation BETWEEN 1 AND 9007199254740991
        AND ((predecessor IS NULL AND generation = 1)
             OR (predecessor BETWEEN 1 AND 9007199254740991
                 AND predecessor = generation - 1))
    ),
    CONSTRAINT c4_generation_epoch_ck CHECK (service_epoch BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c4_generation_blacklist_ck CHECK (public.c4_valid_uuid_array(blacklist_baseline_ids)),
    CONSTRAINT c4_generation_time_ck CHECK (isfinite(created_at)),
    CONSTRAINT c4_generation_phase4_ck CHECK (
        generation = 1 AND purpose <> 'withdrawal_only'
    ),
    CONSTRAINT c4_generation_successor_uq UNIQUE (position_id, predecessor)
);
CREATE UNIQUE INDEX c4_generation_root_uq
    ON public.consignment_disposition_generations (position_id) WHERE predecessor IS NULL;

CREATE TABLE public.consignment_custody_events (
    position_id uuid NOT NULL CONSTRAINT c4_custody_event_position_fk
        REFERENCES public.consignment_custody_positions(id) ON DELETE RESTRICT,
    version bigint NOT NULL,
    command_id uuid NOT NULL CONSTRAINT c4_custody_event_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    event_kind text NOT NULL,
    from_state text,
    to_state text NOT NULL,
    proof_id uuid,
    listing_id uuid NOT NULL CONSTRAINT c4_custody_event_listing_fk
        REFERENCES public.consignment_listings(id) ON DELETE RESTRICT,
    listing_version bigint,
    occurred_at timestamptz NOT NULL,
    CONSTRAINT c4_custody_event_pk PRIMARY KEY (position_id, version),
    -- 事件 `accepted` 对应 position 状态 `awaiting_take`。
    CONSTRAINT c4_custody_event_kind_ck CHECK (event_kind IN (
        'accepted', 'in_custody', 'reconcile', 'committed', 'in_transit',
        'handed_over', 'season_ended', 'lost'
    )),
    CONSTRAINT c4_custody_event_state_ck CHECK ((
        CASE event_kind
            WHEN 'accepted' THEN from_state IS NULL AND to_state = 'awaiting_take'
            WHEN 'in_custody' THEN from_state IN ('awaiting_take', 'reconcile')
                AND to_state = 'in_custody'
            WHEN 'reconcile' THEN from_state IN ('in_custody', 'in_transit') AND to_state = 'reconcile'
            WHEN 'committed' THEN from_state = 'in_custody' AND to_state = 'committed'
            WHEN 'in_transit' THEN from_state = 'committed' AND to_state = 'in_transit'
            WHEN 'handed_over' THEN from_state IN ('in_transit', 'committed') AND to_state = 'handed_over'
            WHEN 'season_ended' THEN from_state IS NOT NULL AND to_state = 'season_ended'
            WHEN 'lost' THEN from_state IS NOT NULL AND to_state = 'lost'
        END
    ) IS TRUE),
    CONSTRAINT c4_custody_event_version_ck CHECK (version BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c4_custody_event_listing_version_ck CHECK (
        listing_version IS NULL OR listing_version BETWEEN 1 AND 9007199254740991
    ),
    CONSTRAINT c4_custody_event_proof_ck CHECK (
        ((event_kind = 'accepted' AND proof_id IS NULL)
        OR (event_kind IN ('in_custody', 'lost') AND public.c4_valid_uuid(proof_id))
        OR (event_kind = 'reconcile' AND public.c4_valid_uuid(proof_id))
        OR (event_kind NOT IN ('accepted', 'in_custody', 'lost', 'reconcile'))) IS TRUE
    ),
    CONSTRAINT c4_custody_event_phase4_ck CHECK (
        event_kind IN ('accepted', 'in_custody', 'reconcile')
    ),
    CONSTRAINT c4_custody_event_time_ck CHECK (isfinite(occurred_at))
);
CREATE INDEX c4_custody_event_command_idx
    ON public.consignment_custody_events (command_id);

CREATE TABLE public.consignment_custody_controls (
    id uuid NOT NULL CONSTRAINT c4_control_pk PRIMARY KEY,
    position_id uuid NOT NULL CONSTRAINT c4_control_position_fk
        REFERENCES public.consignment_custody_positions(id) ON DELETE RESTRICT,
    kind text NOT NULL,
    predecessor_id uuid CONSTRAINT c4_control_predecessor_fk
        REFERENCES public.consignment_custody_controls(id) ON DELETE RESTRICT,
    command_id uuid NOT NULL CONSTRAINT c4_control_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    actor_ref text NOT NULL,
    version bigint NOT NULL,
    occurred_at timestamptz NOT NULL,
    CONSTRAINT c4_control_id_ck CHECK (public.c4_valid_uuid(id)),
    -- 库内控制种类 `withdrawal_intent` 对应 v2 回执处置 `withdrawal_requested`；
    -- 前者是持久事实，后者是回执处置码，值不互换。
    CONSTRAINT c4_control_kind_ck CHECK (kind IN ('withdrawal_intent', 'hold_set', 'hold_released')),
    CONSTRAINT c4_control_actor_ck CHECK (public.consignment_valid_text(actor_ref, 256)),
    CONSTRAINT c4_control_version_ck CHECK (version BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c4_control_time_ck CHECK (isfinite(occurred_at)),
    CONSTRAINT c4_control_version_uq UNIQUE (position_id, version),
    CONSTRAINT c4_control_predecessor_uq UNIQUE (predecessor_id)
);
CREATE UNIQUE INDEX c4_control_active_intent_uq
    ON public.consignment_custody_controls (position_id) WHERE kind = 'withdrawal_intent';

CREATE TABLE public.consignment_equipment_handoffs (
    id uuid NOT NULL CONSTRAINT c4_handoff_edge_pk PRIMARY KEY,
    equipment_id uuid NOT NULL,
    world_id text NOT NULL,
    canonical_eid text NOT NULL,
    season_id text,
    predecessor_id uuid CONSTRAINT c4_handoff_edge_predecessor_fk
        REFERENCES public.consignment_equipment_handoffs(id) ON DELETE RESTRICT,
    from_user_id uuid CONSTRAINT c4_handoff_edge_from_user_fk
        REFERENCES public.users(id) ON DELETE RESTRICT,
    to_user_id uuid CONSTRAINT c4_handoff_edge_to_user_fk
        REFERENCES public.users(id) ON DELETE RESTRICT,
    kind text NOT NULL,
    auction_lot_id uuid CONSTRAINT c4_handoff_edge_lot_fk
        REFERENCES public.lots(id) ON DELETE RESTRICT,
    position_id uuid CONSTRAINT c4_handoff_edge_position_fk
        REFERENCES public.consignment_custody_positions(id) ON DELETE RESTRICT,
    proof_ref text NOT NULL,
    occurred_at timestamptz NOT NULL,
    -- season_id 在 main 为 NULL；复合 FK 的 MATCH SIMPLE 会整体跳过含 NULL 的引用，
    -- 因此资产身份只绑定三列，季由 c4_handoff_edge_asset_season 触发器核对。
    CONSTRAINT c4_handoff_edge_asset_fk
        FOREIGN KEY (equipment_id, world_id, canonical_eid)
        REFERENCES public.consignment_custody_assets (equipment_id, world_id, canonical_eid)
        ON DELETE RESTRICT,
    CONSTRAINT c4_handoff_edge_id_ck CHECK (public.c4_valid_uuid(id)),
    -- kind 映射：custody_acquired ← 整封 Take 证明成立、position 进入在库；
    -- auction_winner_collected ← 拍卖买家确领（WinnerEquipmentCod / WinnerEquipmentHandoff）；
    -- auction_unsold_returned ← UnsoldReturn；
    -- auction_withdrawn_returned ← WithdrawnReturn；
    -- consignment_delivered / consignment_withdrawn / consignment_recovered ← 第 6、7 期。
    CONSTRAINT c4_handoff_edge_kind_ck CHECK (kind IN (
        'custody_acquired', 'auction_winner_collected', 'auction_unsold_returned',
        'auction_withdrawn_returned', 'consignment_delivered',
        'consignment_withdrawn', 'consignment_recovered'
    )),
    -- 第 4 期只产生前四种；后三种由第 6、7 期以新迁移放宽。
    CONSTRAINT c4_handoff_edge_phase4_ck CHECK (kind IN (
        'custody_acquired', 'auction_winner_collected', 'auction_unsold_returned',
        'auction_withdrawn_returned'
    )),
    CONSTRAINT c4_handoff_edge_source_ck CHECK (
        num_nonnulls(auction_lot_id, position_id) = 1
    ),
    CONSTRAINT c4_handoff_edge_proof_ck CHECK (
        public.consignment_safe_observation_text(proof_ref, 256)
    ),
    CONSTRAINT c4_handoff_edge_time_ck CHECK (isfinite(occurred_at))
);
CREATE UNIQUE INDEX c4_handoff_edge_predecessor_uq
    ON public.consignment_equipment_handoffs (predecessor_id)
    WHERE predecessor_id IS NOT NULL;
CREATE INDEX c4_handoff_edge_identity_idx
    ON public.consignment_equipment_handoffs
        (world_id, canonical_eid, season_id, occurred_at, id);

CREATE TABLE public.consignment_season_notices (
    id uuid NOT NULL CONSTRAINT c4_notice_pk PRIMARY KEY,
    season_window_id uuid NOT NULL CONSTRAINT c4_notice_season_fk
        REFERENCES public.consignment_season_windows(id) ON DELETE RESTRICT,
    position_id uuid NOT NULL,
    seller_user_id uuid NOT NULL,
    kind text NOT NULL,
    command_id uuid NOT NULL CONSTRAINT c4_notice_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    created_at timestamptz NOT NULL,
    CONSTRAINT c4_notice_position_fk FOREIGN KEY (position_id, seller_user_id)
        REFERENCES public.consignment_custody_positions (id, seller_user_id) ON DELETE RESTRICT,
    CONSTRAINT c4_notice_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c4_notice_kind_ck CHECK (kind IN ('preclose_withdrawal', 'season_close')),
    CONSTRAINT c4_notice_time_ck CHECK (isfinite(created_at)),
    CONSTRAINT c4_notice_business_uq UNIQUE (position_id, seller_user_id, kind)
);

CREATE TABLE public.mail_disposition_work (
    id uuid NOT NULL CONSTRAINT c4_work_pk PRIMARY KEY,
    bot_uid text,
    world_id text,
    external_mid text,
    decision_id uuid,
    notice_id uuid CONSTRAINT c4_work_notice_fk
        REFERENCES public.consignment_season_notices(id) ON DELETE RESTRICT,
    action text NOT NULL,
    delivery_id uuid NOT NULL CONSTRAINT c4_work_delivery_uq UNIQUE
        CONSTRAINT c4_work_delivery_fk REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    expected_observation_id uuid,
    manifest_digest bytea,
    created_at timestamptz NOT NULL,
    CONSTRAINT c4_work_decision_fk
        FOREIGN KEY (bot_uid, world_id, external_mid, decision_id)
        REFERENCES public.mail_final_dispositions (bot_uid, world_id, external_mid, id)
        ON DELETE RESTRICT,
    CONSTRAINT c4_work_observation_fk
        FOREIGN KEY (bot_uid, world_id, external_mid, expected_observation_id)
        REFERENCES public.mail_envelope_observations (bot_uid, world_id, external_mid, id)
        ON DELETE RESTRICT,
    CONSTRAINT c4_work_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c4_work_action_ck CHECK (action IN ('take', 'return', 'send')),
    CONSTRAINT c4_work_shape_ck CHECK ((
        CASE
            WHEN action IN ('take', 'return') THEN
                bot_uid IS NOT NULL AND world_id IS NOT NULL AND external_mid IS NOT NULL
                AND decision_id IS NOT NULL AND notice_id IS NULL
                AND public.c4_valid_uuid(expected_observation_id)
                AND public.c4_valid_digest(manifest_digest)
            WHEN action = 'send' THEN
                bot_uid IS NULL AND world_id IS NULL AND external_mid IS NULL
                AND decision_id IS NULL AND notice_id IS NOT NULL
                AND expected_observation_id IS NULL AND manifest_digest IS NULL
        END
    ) IS TRUE),
    CONSTRAINT c4_work_time_ck CHECK (isfinite(created_at))
);
CREATE UNIQUE INDEX c4_work_decision_uq
    ON public.mail_disposition_work (decision_id) WHERE decision_id IS NOT NULL;
CREATE UNIQUE INDEX c4_work_key_uq
    ON public.mail_disposition_work (bot_uid, world_id, external_mid) WHERE bot_uid IS NOT NULL;
CREATE UNIQUE INDEX c4_work_notice_uq
    ON public.mail_disposition_work (notice_id) WHERE notice_id IS NOT NULL;

CREATE TABLE public.mail_disposition_attempts (
    id uuid NOT NULL CONSTRAINT c4_attempt_pk PRIMARY KEY,
    work_id uuid NOT NULL CONSTRAINT c4_attempt_work_fk
        REFERENCES public.mail_disposition_work(id) ON DELETE RESTRICT,
    delivery_id uuid NOT NULL CONSTRAINT c4_attempt_delivery_fk
        REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    fence bigint NOT NULL,
    credential_id uuid NOT NULL CONSTRAINT c4_attempt_credential_fk
        REFERENCES public.credential_keys(id) ON DELETE RESTRICT,
    credential_version bigint NOT NULL,
    runtime_generation bigint NOT NULL,
    session_id uuid NOT NULL CONSTRAINT c4_attempt_session_fk
        REFERENCES public.mail_evidence_sessions(id) ON DELETE RESTRICT,
    expected_digest bytea NOT NULL,
    prepared_at timestamptz NOT NULL,
    CONSTRAINT c4_attempt_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c4_attempt_versions_ck CHECK (
        fence BETWEEN 1 AND 9007199254740991
        AND credential_version BETWEEN 1 AND 9007199254740991
        AND runtime_generation BETWEEN 1 AND 9007199254740991
    ),
    CONSTRAINT c4_attempt_digest_ck CHECK (public.c4_valid_digest(expected_digest)),
    CONSTRAINT c4_attempt_time_ck CHECK (isfinite(prepared_at)),
    CONSTRAINT c4_attempt_delivery_fence_uq UNIQUE (delivery_id, fence),
    CONSTRAINT c4_attempt_identity_uq UNIQUE (work_id, delivery_id, id, fence)
);

CREATE TABLE public.mail_disposition_write_starts (
    work_id uuid NOT NULL CONSTRAINT c4_start_pk PRIMARY KEY
        CONSTRAINT c4_start_work_fk
        REFERENCES public.mail_disposition_work(id) ON DELETE RESTRICT,
    decision_id uuid CONSTRAINT c4_start_decision_uq UNIQUE
        CONSTRAINT c4_start_decision_fk
        REFERENCES public.mail_final_dispositions(id) ON DELETE RESTRICT,
    delivery_id uuid NOT NULL CONSTRAINT c4_start_delivery_uq UNIQUE
        CONSTRAINT c4_start_delivery_fk REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    attempt_id uuid NOT NULL CONSTRAINT c4_start_attempt_uq UNIQUE,
    fence bigint NOT NULL,
    started_at timestamptz NOT NULL,
    command_id uuid NOT NULL CONSTRAINT c4_start_command_uq UNIQUE
        CONSTRAINT c4_start_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT c4_start_attempt_fk FOREIGN KEY (work_id, delivery_id, attempt_id, fence)
        REFERENCES public.mail_disposition_attempts (work_id, delivery_id, id, fence)
        ON DELETE RESTRICT,
    CONSTRAINT c4_start_version_ck CHECK (fence BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c4_start_time_ck CHECK (isfinite(started_at)),
    CONSTRAINT c4_start_identity_uq UNIQUE (work_id, delivery_id, attempt_id, fence)
);

CREATE TABLE public.mail_disposition_proofs (
    id uuid NOT NULL CONSTRAINT c4_proof_pk PRIMARY KEY,
    work_id uuid NOT NULL CONSTRAINT c4_proof_work_fk
        REFERENCES public.mail_disposition_work(id) ON DELETE RESTRICT,
    decision_id uuid CONSTRAINT c4_proof_decision_fk
        REFERENCES public.mail_final_dispositions(id) ON DELETE RESTRICT,
    delivery_id uuid NOT NULL,
    fence bigint NOT NULL,
    attempt_id uuid NOT NULL,
    kind text NOT NULL,
    asset_leg text NOT NULL,
    observation_id uuid NOT NULL CONSTRAINT c4_proof_observation_fk
        REFERENCES public.mail_envelope_observations(id) ON DELETE RESTRICT,
    validator_version text NOT NULL,
    digest bytea NOT NULL,
    conclusion text NOT NULL,
    proven_at timestamptz NOT NULL,
    CONSTRAINT c4_proof_start_fk FOREIGN KEY (work_id, delivery_id, attempt_id, fence)
        REFERENCES public.mail_disposition_write_starts (work_id, delivery_id, attempt_id, fence)
        ON DELETE RESTRICT,
    CONSTRAINT c4_proof_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c4_proof_kind_ck CHECK (kind IN ('exact_take', 'exact_return')),
    CONSTRAINT c4_proof_leg_ck CHECK (asset_leg IN ('original', 'carrier', 'cash')),
    CONSTRAINT c4_proof_conclusion_ck CHECK (
        conclusion IN ('take_proven', 'return_proven', 'evidence_quarantined')
    ),
    CONSTRAINT c4_proof_take_decision_ck CHECK (
        conclusion <> 'take_proven' OR decision_id IS NOT NULL
    ),
    CONSTRAINT c4_proof_validator_ck CHECK (public.consignment_valid_text(validator_version, 64)),
    CONSTRAINT c4_proof_digest_ck CHECK (public.c4_valid_digest(digest)),
    CONSTRAINT c4_proof_time_ck CHECK (isfinite(proven_at))
);
-- 只有结论性证明占唯一槽；evidence_quarantined 可多条且只追加，迟补正证不被烧掉。
CREATE UNIQUE INDEX c4_proof_conclusion_uq
    ON public.mail_disposition_proofs (work_id, asset_leg)
    WHERE conclusion IN ('take_proven', 'return_proven');
CREATE UNIQUE INDEX c4_proof_take_uq
    ON public.mail_disposition_proofs (decision_id)
    WHERE conclusion = 'take_proven' AND decision_id IS NOT NULL;
CREATE UNIQUE INDEX c4_proof_take_delivery_fence_uq
    ON public.mail_disposition_proofs (delivery_id, fence)
    WHERE conclusion = 'take_proven';

-- 成环引用在全部表建立后补齐，且延迟到提交：整封接受、position 与唯一 Take 必须在同一
-- 顶层事务内同时可见。
ALTER TABLE public.mail_quarantines
    ADD CONSTRAINT c4_quarantine_id_version_uq UNIQUE (id, version);

-- 观察的隔离版本只在写入时对齐当前投影；隔离版本随后抬升不能让历史观察失效。
ALTER TABLE public.mail_envelope_observations
    ADD CONSTRAINT c4_observation_quarantine_id_fk
        FOREIGN KEY (quarantine_id)
        REFERENCES public.mail_quarantines (id)
        ON DELETE RESTRICT;

ALTER TABLE public.mail_initial_decisions
    ADD CONSTRAINT c4_initial_quarantine_fk
        FOREIGN KEY (quarantine_id) REFERENCES public.mail_quarantines (id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE public.consignment_equipment_claims
    ADD CONSTRAINT c4_claim_position_fk
        FOREIGN KEY (position_id, world_id, canonical_eid)
        REFERENCES public.consignment_custody_positions (id, world_id, canonical_eid)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE public.consignment_custody_positions
    ADD CONSTRAINT c4_position_generation_fk
        FOREIGN KEY (id, current_generation)
        REFERENCES public.consignment_disposition_generations (position_id, generation)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE public.consignment_disposition_generations
    ADD CONSTRAINT c4_generation_proof_fk
        FOREIGN KEY (proof_id) REFERENCES public.mail_disposition_proofs (id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE public.consignment_custody_events
    ADD CONSTRAINT c4_custody_event_proof_fk
        FOREIGN KEY (proof_id) REFERENCES public.mail_disposition_proofs (id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED;

-- 挂单原因函数扩展为全集；旧分类的返回值保持不变。
-- `rejected` 与 `rejected_with_snapshot` 是同一领域拒绝的两种投影；分流由
-- `current_snapshot_id` 是否有值决定。
CREATE OR REPLACE FUNCTION public.consignment_reason_state(reason text) RETURNS text
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog
AS $function$
    SELECT CASE
        WHEN reason IN ('not_tradeable', 'owner_mismatch', 'history_owner_mismatch',
                        'history_alias_conflict', 'history_unproven', 'equipment_busy')
            THEN 'rejected_with_snapshot'
        WHEN reason IN ('equipment_not_found', 'source_request_rejected', 'source_http_invalid',
                        'source_schema_invalid', 'source_too_large', 'source_shape_unconfirmed',
                        'source_identity_mismatch', 'owner_missing', 'credential_unavailable',
                        'lookup_exhausted') THEN 'rejected'
        WHEN reason IN ('lookup_deadline', 'display_expired', 'deposit_expired') THEN 'expired'
        WHEN reason IN ('auction_lot_created', 'seller_ineligible', 'service_disabled',
                        'seller_cancelled', 'admin_delisted', 'sold', 'withdrawn_delivered',
                        'season_closed', 'loss_confirmed') THEN 'closed'
    END
$function$;

-- 既有命令行扩展 schema_version 与类型化目标。v1 分支原样保留。
ALTER TABLE public.consignment_commands
    ADD COLUMN schema_version smallint NOT NULL DEFAULT 1,
    ADD COLUMN target_kind text,
    ADD COLUMN target_id uuid,
    ADD COLUMN target_bot_uid text,
    ADD COLUMN target_world_id text,
    ADD COLUMN target_external_mid text,
    ADD COLUMN target_season_id text,
    ADD COLUMN target_calendar_version bytea;

ALTER TABLE public.consignment_commands
    ADD CONSTRAINT c4_command_schema_version_ck CHECK (schema_version IN (1, 2)),
    ADD CONSTRAINT c4_command_typed_target_ck CHECK ((
        CASE schema_version
            WHEN 1 THEN target_kind IS NULL AND target_id IS NULL
                AND target_bot_uid IS NULL AND target_world_id IS NULL
                AND target_external_mid IS NULL AND target_season_id IS NULL
                AND target_calendar_version IS NULL
            WHEN 2 THEN target_kind IN (
                'listing', 'reservation', 'quarantine', 'mail_key',
                'final_decision', 'original_attempt', 'season')
                AND CASE target_kind
                    WHEN 'mail_key' THEN target_id IS NULL
                        AND target_bot_uid COLLATE "C" ~ '^[1-9][0-9]{0,31}$'
                        AND target_world_id IN ('main', 'isekai')
                        AND target_external_mid COLLATE "C" ~ '^[1-9][0-9]{0,31}$'
                        AND target_season_id IS NULL AND target_calendar_version IS NULL
                    WHEN 'season' THEN target_id IS NULL AND target_bot_uid IS NULL
                        AND target_world_id IS NULL AND target_external_mid IS NULL
                        AND public.consignment_valid_text(target_season_id, 128)
                        AND public.c4_valid_digest(target_calendar_version)
                    ELSE public.c4_valid_uuid(target_id)
                        AND target_bot_uid IS NULL AND target_world_id IS NULL
                        AND target_external_mid IS NULL AND target_season_id IS NULL
                        AND target_calendar_version IS NULL
                END
        END
    ) IS TRUE);

ALTER TABLE public.consignment_commands
    DROP CONSTRAINT consignment_command_actor;
ALTER TABLE public.consignment_commands
    ADD CONSTRAINT consignment_command_actor CHECK ((
        CASE
            WHEN schema_version = 1 THEN CASE actor_kind
                WHEN 'public_user' THEN operation IN ('register', 'reprice', 'withdraw')
                    AND public.consignment_positive_uuid(actor_ref)
                WHEN 'admin' THEN operation = 'admin_delist'
                    AND public.consignment_valid_text(actor_ref, 256) AND strpos(actor_ref, ':') = 0
                WHEN 'worker' THEN operation = 'apply_lookup' AND public.consignment_valid_text(actor_ref, 256)
                WHEN 'system' THEN operation = 'auto_close' AND public.consignment_valid_text(actor_ref, 256)
            END
            WHEN schema_version = 2 THEN CASE actor_kind
                WHEN 'public_user' THEN operation IN ('issue_reservation', 'withdraw')
                    AND public.consignment_positive_uuid(actor_ref)
                WHEN 'admin' THEN operation IN (
                    'recheck_quarantine', 'release_quarantine', 'admin_delist', 'release_hold')
                    AND public.consignment_valid_text(actor_ref, 256) AND strpos(actor_ref, ':') = 0
                WHEN 'worker' THEN operation IN (
                    'expire_reservation', 'decide_envelope', 'append_quarantine_observation',
                    'start_disposition_write', 'apply_mail_proof', 'season_notice', 'season_close')
                    AND public.consignment_valid_text(actor_ref, 256)
                WHEN 'system' THEN operation IN ('release_quarantine', 'recheck_quarantine')
                    AND public.consignment_valid_text(actor_ref, 256)
            END
        END
    ) IS TRUE);

ALTER TABLE public.consignment_commands
    DROP CONSTRAINT consignment_command_target;
ALTER TABLE public.consignment_commands
    ADD CONSTRAINT consignment_command_target CHECK (
        (
            schema_version = 1 AND (
                (operation = 'register' AND request_target = 'new' AND replay_key_id IS NOT NULL
                    AND (result_status = 202) = (listing_id IS NOT NULL))
                OR (operation <> 'register' AND public.consignment_positive_uuid(request_target)
                    AND replay_key_id IS NULL
                    AND (listing_id IS NULL OR listing_id::text = request_target))
            )
        )
        OR (
            schema_version = 2 AND (
                (target_kind = 'listing'
                    AND request_target = target_id::text AND listing_id = target_id
                    AND operation IN ('issue_reservation', 'withdraw', 'admin_delist', 'release_hold'))
                OR (target_kind = 'reservation'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation = 'expire_reservation')
                OR (target_kind = 'quarantine'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation IN ('append_quarantine_observation', 'recheck_quarantine',
                                      'release_quarantine'))
                OR (target_kind = 'final_decision'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation = 'start_disposition_write')
                OR (target_kind = 'original_attempt'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation = 'apply_mail_proof')
                OR (target_kind = 'mail_key'
                    AND request_target = target_bot_uid || '|' || target_world_id || '|'
                        || target_external_mid
                    AND listing_id IS NULL AND operation = 'decide_envelope')
                OR (target_kind = 'season'
                    AND request_target = target_season_id AND listing_id IS NULL
                    AND operation IN ('season_notice', 'season_close'))
            )
        )
    );

ALTER TABLE public.consignment_commands
    DROP CONSTRAINT consignment_command_result_code;
ALTER TABLE public.consignment_commands
    ADD CONSTRAINT consignment_command_result_code CHECK (
        (
            schema_version = 1 AND (
                (result_status = 202 AND operation = 'register' AND result_code = 'accepted')
                OR (result_status = 200 AND operation <> 'register' AND result_code = 'ok')
                OR (operation = 'register' AND (
                    (result_status = 403 AND result_code = 'participant_ineligible')
                    OR (result_status = 409 AND result_code IN (
                        'already_open', 'equipment_busy', 'history_unproven', 'history_owner_mismatch',
                        'history_alias_conflict', 'fee_changed', 'world_unavailable'
                    ))
                    OR (result_status = 429 AND result_code IN (
                        'registration_rate_limited', 'listing_limit_reached'))
                ))
                OR (operation = 'apply_lookup' AND result_status = 409
                    AND result_code = 'listing_not_active')
                OR (operation IN ('reprice', 'withdraw', 'admin_delist') AND (
                    (result_status = 404 AND result_code = 'listing_not_found')
                    OR (result_status = 409 AND result_code IN ('stale_version', 'listing_not_active'))
                ))
            )
        )
        OR (
            schema_version = 2 AND (
                (result_status IN (200, 201, 202) AND result_code = 'ok')
                OR (result_status = 403 AND result_code IN (
                    'participant_ineligible', 'bot_party', 'action_not_allowed'))
                OR (result_status = 404 AND result_code = 'consignment_disabled')
                OR (result_status = 409 AND result_code IN (
                    'stale_version', 'listing_not_active', 'equipment_busy', 'equipment_committed',
                    'custody_unproven', 'operations_hold', 'season_closed', 'hold_mismatch',
                    'source_mismatch', 'evidence_unproven', 'evidence_conflict',
                    'disposition_conflict', 'fence_mismatch'))
                OR (result_status = 429 AND result_code = 'listing_limit_reached')
                OR (result_status = 503 AND result_code = 'mail_unavailable')
            )
        )
    );

ALTER TABLE public.consignment_commands
    DROP CONSTRAINT consignment_command_receipt;
ALTER TABLE public.consignment_commands
    ADD CONSTRAINT consignment_command_receipt CHECK (
        (schema_version = 1
            AND public.consignment_valid_receipt(
                result_body, command_id, operation, listing_id, result_status))
        OR (schema_version = 2
            AND public.c4_valid_v2_receipt(
                result_body, command_id, operation, result_status))
    );

-- 全部挂单状态与 held_off_market 原因闭集。第 4 期可产生 resolving/displayed/
-- awaiting_deposit/deposit_accepted/available/held_off_market/withdrawal_pending，
-- 以及含季末原因的 closed 与旧终态。
ALTER TABLE public.consignment_listings
    DROP CONSTRAINT consignment_listing_state_shape;
ALTER TABLE public.consignment_listings
    ADD CONSTRAINT consignment_listing_state_shape CHECK ((
        CASE state
            WHEN 'resolving' THEN reason_code IS NULL AND current_snapshot_id IS NULL
            WHEN 'displayed' THEN reason_code IS NULL AND mode = 'direct'
                AND current_snapshot_id IS NOT NULL
            WHEN 'awaiting_deposit' THEN reason_code IS NULL AND mode = 'consigned'
                AND current_snapshot_id IS NOT NULL
            WHEN 'deposit_accepted' THEN reason_code IS NULL AND mode = 'consigned'
                AND current_snapshot_id IS NOT NULL
            WHEN 'available' THEN reason_code IS NULL AND mode = 'consigned'
                AND current_snapshot_id IS NOT NULL
            WHEN 'sale_pending' THEN reason_code IS NULL AND mode = 'consigned'
                AND current_snapshot_id IS NOT NULL
            WHEN 'held_off_market' THEN reason_code IN (
                'unpaid_recovered', 'paid_returned', 'seller_ineligible', 'service_disabled',
                'quota_blocked', 'operations_hold', 'review')
                AND mode = 'consigned' AND current_snapshot_id IS NOT NULL
            WHEN 'withdrawal_pending' THEN reason_code IS NULL AND mode = 'consigned'
                AND current_snapshot_id IS NOT NULL
            WHEN 'rejected' THEN public.consignment_reason_state(reason_code) =
                CASE WHEN current_snapshot_id IS NULL THEN 'rejected' ELSE 'rejected_with_snapshot' END
            WHEN 'expired' THEN public.consignment_reason_state(reason_code) = 'expired'
            WHEN 'closed' THEN public.consignment_reason_state(reason_code) = 'closed'
        END
    ) IS TRUE);

DROP INDEX public.consignment_listings_seller_active_idx;
CREATE UNIQUE INDEX consignment_listings_seller_active_idx
    ON public.consignment_listings (seller_user_id, world_id, external_equipment_id)
    WHERE state IN ('resolving', 'displayed', 'awaiting_deposit', 'available');
DROP INDEX public.consignment_listings_equipment_published_idx;
CREATE UNIQUE INDEX consignment_listings_equipment_published_idx
    ON public.consignment_listings (world_id, external_equipment_id)
    WHERE state IN ('displayed', 'awaiting_deposit', 'available');

-- 事件边扩展第 4 期可产生的代管迁移；sale_pending 与成交、交付撤回、损失等后期关闭原因
-- 尚未开写，留给对应期以新迁移放宽。
ALTER TABLE public.consignment_events
    DROP CONSTRAINT consignment_event_edge;
ALTER TABLE public.consignment_events
    ADD CONSTRAINT consignment_event_edge CHECK ((
        CASE
            WHEN state_version = 1 THEN event_kind = 'registered' AND actor_kind = 'public_user'
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
                    WHEN 'season_closed' THEN actor_kind IN ('worker', 'system')
                        AND to_state = 'closed' AND reason_code = 'season_closed'
                END
            WHEN from_state = 'available' THEN
                CASE event_kind
                    WHEN 'repriced' THEN actor_kind = 'public_user'
                        AND to_state = 'available' AND reason_code IS NULL
                    WHEN 'hold_set' THEN actor_kind = 'admin' AND to_state = 'held_off_market'
                        AND reason_code IN ('unpaid_recovered', 'paid_returned', 'seller_ineligible',
                                            'service_disabled', 'quota_blocked', 'operations_hold',
                                            'review')
                    WHEN 'withdrawal_requested' THEN actor_kind = 'public_user'
                        AND to_state = 'withdrawal_pending' AND reason_code IS NULL
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
                    WHEN 'season_closed' THEN actor_kind IN ('worker', 'system')
                        AND to_state = 'closed' AND reason_code = 'season_closed'
                END
            WHEN from_state = 'withdrawal_pending' THEN
                CASE event_kind
                    WHEN 'season_closed' THEN actor_kind IN ('worker', 'system')
                        AND to_state = 'closed' AND reason_code = 'season_closed'
                END
        END
    ) IS TRUE);

-- 投递来源只追加一个新分支；旧来源、旧载荷与旧解码不变。
ALTER TABLE public.deliveries
    DROP CONSTRAINT deliveries_origin_kind_check;
ALTER TABLE public.deliveries
    ADD CONSTRAINT deliveries_origin_kind_check CHECK (
        origin_kind IN (
            'settlement',
            'settlement_recovery_action',
            'settlement_return_action',
            'user_key',
            'manual',
            'mail_intake',
            'operations_alert',
            'lot_watch',
            'seller_summary',
            'season_reminder',
            'consignment_disposition',
            'consignment_notice'
        )
    );
ALTER TABLE public.deliveries
    ADD CONSTRAINT c4_delivery_consignment_payload_ck CHECK (
        (origin_kind <> 'consignment_disposition'
        OR (
            payload->'schema_version' = '2'::jsonb
            AND payload->>'operation' IN ('take', 'return')
            AND jsonb_typeof(payload->'mail_key') = 'object'
            AND payload->'mail_key' ? 'bot_uid'
            AND payload->'mail_key' ? 'world_id'
            AND payload->'mail_key' ? 'external_mid'
            AND payload->'mail_key'->>'bot_uid' COLLATE "C" ~ '^[1-9][0-9]{0,31}$'
            AND payload->'mail_key'->>'world_id' IN ('main', 'isekai')
            AND payload->'mail_key'->>'external_mid' COLLATE "C" ~ '^[1-9][0-9]{0,31}$'
            AND payload->'mail_key'->>'world_id' = payload->>'world'
            AND payload->'mail_key'->>'external_mid' = payload->>'external_mail_id'
            AND public.c4_valid_json_uuid(payload->'decision_id')
            AND public.c4_valid_json_uuid(payload->'work_id')
        )) IS TRUE
    );

-- 寄售处置载荷的完整形状 = 投递通用外壳键（world、external_mail_id、template、
-- result_code、body、user_key_user_id、credits、cod、attachment_ids，与其他投递来源
-- 相同）+ 寄售专有键（schema_version=2、operation、mail_key、decision_id、work_id、
-- attempt_id、expected_manifest、expected_return）。mail_key.world_id 必须等于外壳
-- world，mail_key.external_mid 必须等于外壳 external_mail_id；旧来源仍用 schema 1，
-- 不得携带寄售专有键。
COMMENT ON TABLE public.deliveries IS
    '投递 outbox。origin_kind=consignment_disposition 的 payload 由投递通用外壳键（world、external_mail_id、template、result_code、body、user_key_user_id、credits、cod、attachment_ids）与寄售专有键（schema_version=2、operation、mail_key、decision_id、work_id、attempt_id、expected_manifest、expected_return）组成；mail_key.world_id 必须等于 world，mail_key.external_mid 必须等于 external_mail_id。';

-- 季前通知只走通用 Send 外壳：无附件、无 COD、无装备引用；正文与季末时刻在入队时冻结。
ALTER TABLE public.deliveries
    ADD CONSTRAINT c4_delivery_consignment_notice_payload_ck CHECK (
        (origin_kind <> 'consignment_notice'
        OR (
            payload->'schema_version' = '1'::jsonb
            AND payload->>'operation' = 'send'
            AND payload->>'template' = 'consignment_season_notice'
            AND payload->>'world' = 'isekai'
            AND payload->'credits' = '0'::jsonb
            AND payload->'cod' = '0'::jsonb
            AND jsonb_typeof(payload->'attachment_ids') = 'array'
            AND payload->'attachment_ids' = '[]'::jsonb
            AND NOT payload ? 'equipment_secret_refs'
            AND jsonb_typeof(payload->'body') = 'string'
        )) IS TRUE
    );

-- 季通知与换季提醒同归结算类节奏；判定顺序与 000098 完全一致，只把新来源并列到结算分支。
-- 函数体只用内置类型与运算符；仍显式固定 search_path，会话搜索路径不能把同名运算符解析到别处
-- （表达式索引 deliveries_admin_category_idx 依赖本函数，重建前必须保持同一解析结果）。
CREATE OR REPLACE FUNCTION public.delivery_send_category(origin text, payload jsonb)
RETURNS text LANGUAGE sql IMMUTABLE PARALLEL SAFE SET search_path = pg_catalog AS $$
    SELECT CASE
        WHEN payload ->> 'operation' IS DISTINCT FROM 'send' THEN NULL
        WHEN origin = 'lot_watch' THEN 'outbid'
        WHEN origin = 'operations_alert' THEN 'operations'
        WHEN origin IN ('seller_summary', 'season_reminder', 'consignment_notice')
            THEN 'settlement'
        WHEN origin = 'mail_intake' AND payload ->> 'template'
            IN (
                'register_key', 'register_result', 'rekey_key', 'rekey_result',
                'rebind_result', 'user_assets_rejected'
            )
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

REINDEX INDEX public.deliveries_admin_category_idx;

-- 挂单守卫：保留 000132 的登记、冻结字段与旧状态迁移；新增代管态只允许第 4 期可产生的边。
-- 不变量 3、6、10：版本、快照、价格与期限受保护；未来期的成交/撤回/新关闭原因不能提前写入。
CREATE OR REPLACE FUNCTION public.consignment_guard_listing() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF TG_OP = 'INSERT' THEN
        NEW.registration_xact_id := pg_current_xact_id();
        IF NEW.state_version <> 1 OR NEW.state <> 'resolving' THEN
            RAISE EXCEPTION '寄售挂单必须从登记版本开始'
                USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
        END IF;
        RETURN NEW;
    END IF;

    IF ROW(OLD.id, OLD.seller_user_id, OLD.world_id, OLD.external_equipment_id, OLD.mode,
           OLD.pricing_version, OLD.fee_bps, OLD.service_epoch, OLD.blacklist_count_at_register,
           OLD.blacklist_baseline_ids, OLD.created_at, OLD.registration_xact_id)
       IS DISTINCT FROM
       ROW(NEW.id, NEW.seller_user_id, NEW.world_id, NEW.external_equipment_id, NEW.mode,
           NEW.pricing_version, NEW.fee_bps, NEW.service_epoch, NEW.blacklist_count_at_register,
           NEW.blacklist_baseline_ids, NEW.created_at, NEW.registration_xact_id)
       OR OLD.state NOT IN ('resolving', 'displayed', 'awaiting_deposit',
                            'deposit_accepted', 'available', 'held_off_market',
                            'withdrawal_pending')
       OR NEW.state_version <> OLD.state_version + 1
       OR NEW.updated_at < OLD.updated_at THEN
        RAISE EXCEPTION '寄售挂单身份、版本或时间变化不合法'
            USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
    END IF;

    -- 旧解析/展示/待寄期限仍按锁后数据库时刻比较；新代管态保留历史 expires_at 但不作 TTL。
    IF OLD.state IN ('resolving', 'displayed', 'awaiting_deposit')
       AND NEW.updated_at >= OLD.expires_at
       AND NEW.state <> 'expired'
       AND NOT (NEW.state = 'closed' AND NEW.reason_code IN (
           'auction_lot_created', 'seller_ineligible', 'service_disabled',
           'season_closed')) THEN
        RAISE EXCEPTION '寄售挂单在旧截止后不能继续发布或迁移'
            USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
    END IF;

    IF NEW.state = OLD.state THEN
        IF NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
           OR NEW.expires_at <> OLD.expires_at THEN
            RAISE EXCEPTION '寄售同态增版不能替换快照或期限'
                USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
        END IF;
        -- 不变量 6：同态增版不能改写原因。held_off_market 只允许运营止售与解除
        -- 产生的 operations_hold/review，具体命令与回执原因由事件触发器逐项绑定。
        IF NEW.reason_code IS DISTINCT FROM OLD.reason_code
           AND NOT (OLD.state = 'held_off_market'
                    AND NEW.reason_code IN ('operations_hold', 'review')) THEN
            RAISE EXCEPTION '寄售同态增版不能改写挂单原因'
                USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
        END IF;
    ELSIF OLD.state = 'resolving' THEN
        IF NEW.state IN ('displayed', 'awaiting_deposit') THEN
            IF NEW.price <> OLD.price OR NEW.expires_at <= NEW.updated_at THEN
                RAISE EXCEPTION '寄售发布不能改价或用已到期的期限'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSIF NEW.state IN ('rejected', 'expired') THEN
            IF NEW.price <> OLD.price OR NEW.expires_at <> OLD.expires_at THEN
                RAISE EXCEPTION '寄售拒挂或到期不能改价或期限'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
            IF NEW.state = 'expired' AND NEW.updated_at < OLD.expires_at THEN
                RAISE EXCEPTION '寄售解析态不能早于旧截止到期'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSIF NEW.state = 'closed' THEN
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at
               OR NEW.reason_code IS NULL
               OR NEW.reason_code NOT IN (
                   'auction_lot_created', 'seller_ineligible', 'service_disabled',
                   'seller_cancelled', 'admin_delisted', 'season_closed') THEN
                RAISE EXCEPTION '寄售关闭不能改价、快照或期限'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSE
            RAISE EXCEPTION '寄售解析态不能迁移到该状态'
                USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
        END IF;
    ELSIF OLD.state IN ('displayed', 'awaiting_deposit') THEN
        IF NEW.state IN ('expired', 'closed') THEN
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at THEN
                RAISE EXCEPTION '寄售展示态终结不能改价、快照或期限'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
            IF NEW.state = 'expired' AND NEW.updated_at < OLD.expires_at THEN
                RAISE EXCEPTION '寄售展示态不能早于旧截止到期'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSIF OLD.state = 'awaiting_deposit' AND NEW.state = 'deposit_accepted' THEN
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at
               OR NEW.reason_code IS NOT NULL THEN
                RAISE EXCEPTION '寄售整封接受不能改价、快照或期限'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSE
            RAISE EXCEPTION '寄售展示态不能迁移到该状态'
                USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
        END IF;
    ELSIF OLD.state = 'deposit_accepted' THEN
        IF NEW.state = 'withdrawal_pending' THEN
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at
               OR NEW.reason_code IS NOT NULL THEN
                RAISE EXCEPTION '寄售取得后撤回不能改价、快照、期限或原因'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSIF NEW.state = 'available' THEN
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at OR NEW.reason_code IS NOT NULL THEN
                RAISE EXCEPTION '寄售取得后上架不能改价、快照或期限'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSIF NEW.state = 'held_off_market' THEN
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at
               OR NEW.reason_code NOT IN (
                   'unpaid_recovered', 'paid_returned', 'seller_ineligible', 'service_disabled',
                   'quota_blocked', 'operations_hold', 'review') THEN
                RAISE EXCEPTION '寄售取得后下架原因或字段不合法'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSIF NEW.state = 'closed' THEN
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at
               OR NEW.reason_code IS DISTINCT FROM 'season_closed' THEN
                RAISE EXCEPTION '寄售取得后关闭只能因季末且不能改价、快照或期限'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSE
            RAISE EXCEPTION '寄售已接受态不能迁移到该状态'
                USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
        END IF;
    ELSIF OLD.state = 'available' THEN
        IF NEW.state = 'held_off_market' THEN
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at
               OR NEW.reason_code NOT IN (
                   'unpaid_recovered', 'paid_returned', 'seller_ineligible', 'service_disabled',
                   'quota_blocked', 'operations_hold', 'review') THEN
                RAISE EXCEPTION '寄售可售态下架原因或字段不合法'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSIF NEW.state = 'withdrawal_pending' THEN
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at
               OR NEW.reason_code IS NOT NULL THEN
                RAISE EXCEPTION '寄售可售态撤回不能改价、快照、期限或原因'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSIF NEW.state = 'closed' THEN
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at
               OR NEW.reason_code IS DISTINCT FROM 'season_closed' THEN
                RAISE EXCEPTION '寄售可售态关闭只能因季末且不能改价、快照或期限'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSE
            RAISE EXCEPTION '寄售可售态不能迁移到该状态'
                USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
        END IF;
    ELSIF OLD.state = 'held_off_market' THEN
        IF NEW.state = 'withdrawal_pending' THEN
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at
               OR NEW.reason_code IS NOT NULL THEN
                RAISE EXCEPTION '寄售下架态撤回不能改价、快照、期限或原因'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSIF NEW.state = 'closed' THEN
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at
               OR NEW.reason_code IS DISTINCT FROM 'season_closed' THEN
                RAISE EXCEPTION '寄售已下架态关闭只能因季末且不能改价、快照或期限'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSE
            RAISE EXCEPTION '寄售已下架态不能迁移到该状态'
                USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
        END IF;
    ELSIF OLD.state = 'withdrawal_pending' THEN
        IF NEW.state = 'closed' THEN
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at
               OR NEW.reason_code IS DISTINCT FROM 'season_closed' THEN
                RAISE EXCEPTION '寄售撤回态关闭只能因季末且不能改价、快照或期限'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSE
            RAISE EXCEPTION '寄售撤回态不能迁移到该状态'
                USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
        END IF;
    END IF;

    -- 第 4 期不能产生成交、交付撤回与损失关闭；撤回意愿和季末关闭由同事务 v2
    -- 命令与事件逐项绑定。
    IF NEW.state = 'sale_pending'
       OR (NEW.state = 'closed' AND NEW.reason_code IN (
           'sold', 'withdrawn_delivered', 'loss_confirmed')) THEN
        RAISE EXCEPTION '第 4 期不能产生该挂单迁移'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_listing_phase4_transition';
    END IF;
    RETURN NEW;
END
$function$;

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
                                AND c.actor_ref = NEW.seller_user_id::text
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
                        AND c.result_body->>'listing_id' = NEW.id::text
                        AND c.result_body->>'listing_version' = NEW.state_version::text
                        AND c.result_body->>'state' = NEW.state
                        AND c.result_body->>'disposition' = 'withdrawal_requested'
                        AND NEW.reason_code IS NOT DISTINCT FROM previous_reason
                    WHEN 'closed' THEN
                        (
                            (c.operation = 'withdraw' AND c.actor_kind = 'public_user'
                             AND c.listing_id = NEW.id)
                            OR
                            (c.operation = 'admin_delist' AND c.actor_kind = 'admin'
                             AND c.listing_id = NEW.id)
                        )
                        AND c.result_body->>'listing_id' = NEW.id::text
                        AND c.result_body->>'listing_version' = NEW.state_version::text
                        AND c.result_body->>'state' = NEW.state
                        AND c.result_body->>'disposition' = 'closed'
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

-- 成功回执必须与同版本事件同事务；v2 的止售与预约同态增版也走这条检查。
CREATE OR REPLACE FUNCTION public.consignment_check_command_event() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM public.consignment_events AS e
        WHERE e.command_id = NEW.command_id AND e.listing_id = NEW.listing_id
          AND e.state_version = (NEW.result_body->>'listing_version')::bigint
          AND e.event_kind = NEW.result_body->>'disposition'
          AND e.xmin = pg_current_xact_id()::xid
    ) THEN
        RAISE EXCEPTION '寄售成功回执缺少同事务的匹配事件'
            USING ERRCODE = '23514', CONSTRAINT = 'consignment_command_version_event';
    END IF;
    RETURN NULL;
END
$function$;

DROP TRIGGER IF EXISTS consignment_command_version_event ON public.consignment_commands;
CREATE CONSTRAINT TRIGGER consignment_command_version_event
AFTER INSERT ON public.consignment_commands DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW
WHEN (NEW.result_status < 400 AND NEW.result_body->>'disposition' IN
    ('registered', 'repriced', 'closed', 'published', 'rejected', 'expired',
     'reservation_issued', 'hold_set', 'hold_released'))
EXECUTE FUNCTION public.consignment_check_command_event();

-- 事实表只追加：与基线相同的触发器函数与命名后缀。投影表不在此列表。
DO $block$
DECLARE
    table_name text;
BEGIN
    FOREACH table_name IN ARRAY ARRAY[
        'consignment_season_windows',
        'mail_evidence_sessions',
        'mail_originals',
        'mail_original_links',
        'mail_envelope_observations',
        'mail_observation_conflicts',
        'mail_initial_decisions',
        'mail_final_dispositions',
        'mail_quarantine_releases',
        'settlement_recovery_return_handoffs',
        'consignment_custody_assets',
        'consignment_deposit_items',
        'consignment_disposition_generations',
        'consignment_custody_events',
        'consignment_custody_controls',
        'consignment_equipment_handoffs',
        'consignment_season_notices',
        'mail_disposition_work',
        'mail_disposition_attempts',
        'mail_disposition_write_starts',
        'mail_disposition_proofs'
    ] LOOP
        EXECUTE format(
            'CREATE TRIGGER %I BEFORE UPDATE OR DELETE ON public.%I
             FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation()',
            table_name || '_append_only', table_name
        );
        EXECUTE format(
            'CREATE TRIGGER %I BEFORE TRUNCATE ON public.%I
             FOR EACH STATEMENT EXECUTE FUNCTION public.consignment_reject_fact_mutation()',
            table_name || '_no_truncate', table_name
        );
    END LOOP;
END
$block$;

-- 投影守卫：只允许合同列上的前进迁移；信封退休后不可恢复，隔离只能 active→released。
CREATE OR REPLACE FUNCTION public.c4_guard_quarantine() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.state <> 'active' OR NEW.version <> 1 THEN
            RAISE EXCEPTION '隔离必须从 active 版本 1 开始'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_quarantine_transition';
        END IF;
        RETURN NEW;
    END IF;
    IF OLD.state = 'released' AND OLD IS DISTINCT FROM NEW THEN
        RAISE EXCEPTION '已解除隔离不能再修改'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_quarantine_transition';
    END IF;
    IF NEW.id <> OLD.id OR NEW.bot_uid <> OLD.bot_uid OR NEW.world_id <> OLD.world_id
       OR NEW.external_mid <> OLD.external_mid
       OR NEW.initial_decision_id <> OLD.initial_decision_id
       OR NEW.version <> OLD.version + 1
       OR NEW.updated_at < OLD.updated_at
       OR NEW.state NOT IN ('active', 'released') THEN
        RAISE EXCEPTION '隔离版本、身份或状态变化不合法'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_quarantine_transition';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c4_quarantine_transition
BEFORE INSERT OR UPDATE ON public.mail_quarantines
FOR EACH ROW EXECUTE FUNCTION public.c4_guard_quarantine();

CREATE OR REPLACE FUNCTION public.c4_guard_observation_quarantine() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    current_version bigint;
BEGIN
    IF NEW.quarantine_id IS NULL OR NEW.quarantine_version IS NULL THEN
        RETURN NEW;
    END IF;
    SELECT version INTO current_version
    FROM public.mail_quarantines WHERE id = NEW.quarantine_id;
    IF current_version IS NULL OR current_version <> NEW.quarantine_version THEN
        RAISE EXCEPTION '隔离观察版本必须对齐当前隔离投影'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_observation_quarantine_version';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c4_observation_quarantine_version
BEFORE INSERT OR UPDATE OF quarantine_id, quarantine_version
ON public.mail_envelope_observations
FOR EACH ROW EXECUTE FUNCTION public.c4_guard_observation_quarantine();

CREATE OR REPLACE FUNCTION public.c4_guard_readiness() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.version <> 1 THEN
            RAISE EXCEPTION '就绪投影必须从版本 1 开始'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_readiness_transition';
        END IF;
        RETURN NEW;
    END IF;
    IF NEW.world_id <> OLD.world_id OR NEW.version <> OLD.version + 1
       OR NEW.checked_at < OLD.checked_at THEN
        RAISE EXCEPTION '就绪投影版本或世界不可回退'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_readiness_transition';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c4_readiness_transition
BEFORE INSERT OR UPDATE ON public.consignment_mail_readiness
FOR EACH ROW EXECUTE FUNCTION public.c4_guard_readiness();

-- 预约退休/消费的命令绑定对照表。operation 名只取 P4A `DepositOperation` 闭集；
-- 目标列取 `consignment_commands` 的 v2 类型化目标（下表 v1 行按其 listing_id 目标）。
-- 表外组合（含未列出的 v1 命令、失败命令、目标不符或原因不符）一律拒绝
-- （不变量 1、2、6、8）。
--
--   state      reason             允许 operation          目标与同事务事实
--   retired    expired            expire_reservation      target_kind='reservation' 且 target_id=本预约
--   retired    identity_lost      expire_reservation      同上
--   retired    service_disabled   expire_reservation      同上
--   retired    cancelled          withdraw                target_kind='listing' 且 target_id=本预约.listing_id
--   retired    cancelled          admin_delist            同上
--   retired    identity_lost      admin_delist            同上
--   retired    service_disabled   admin_delist            同上
--   retired    reissued           issue_reservation       target_kind='listing' 且 target_id=本预约.listing_id；
--                                                       换预约分支在提交前复核 generation+1 后继预约
--   consumed   —                  decide_envelope          target_kind='mail_key'，回执 disposition='accepted'
--   consumed   —                  release_quarantine       target_kind='quarantine'，回执 decision_kind='accept'
--                                                       两种消费都要求同事务 item 行消费本预约
--
-- “改登”沿旧卖家关闭路径由 `withdraw` 表达；停用/身份失效由 Admin 的 `admin_delist`
-- 或 Worker 的 `expire_reservation` 表达。卖家取消只能由同事务的挂单命令
-- （withdraw / admin_delist，目标为该挂单）随附退休；expire_reservation 只承担
-- Worker 发现的到期、身份失效与停用，不能单独声明取消，否则取消事实没有真实
-- 挂单命令背书（不变量 2、6、8）。v1 的 `withdraw`/`admin_delist` 按其
-- listing_id 目标接受；v1 的 `register`/`reprice`/`apply_lookup`/`auto_close`
-- 不会为 000134 新建的预约提供解释，因此不在表内。
CREATE OR REPLACE FUNCTION public.c4_reservation_retirement_command_ok(
    p_reservation_id uuid,
    p_listing_id uuid,
    p_state text,
    p_reason text,
    p_command_id uuid
) RETURNS boolean
LANGUAGE sql SET search_path = pg_catalog
AS $function$
    SELECT EXISTS (
        SELECT 1
        FROM public.consignment_commands AS command_row
        WHERE command_row.command_id = p_command_id
          AND command_row.xmin = pg_current_xact_id()::xid
          AND command_row.result_status < 400
          AND command_row.result_code = 'ok'
          AND (
              (p_state = 'retired'
               AND p_reason IN ('expired', 'identity_lost', 'service_disabled')
               AND command_row.operation = 'expire_reservation'
               AND command_row.target_kind = 'reservation'
               AND command_row.target_id = p_reservation_id
               AND command_row.result_body->>'disposition' = 'reservation_retired'
               AND command_row.result_body->>'reservation_id' = p_reservation_id::text)
              OR (p_state = 'retired' AND p_reason = 'cancelled'
                  AND (
                      (command_row.schema_version = 2
                       AND command_row.operation = 'withdraw'
                       AND command_row.target_kind = 'listing'
                       AND command_row.target_id = p_listing_id
                       AND command_row.result_body->>'disposition' IN (
                           'withdrawal_requested', 'closed')
                       AND command_row.result_body->>'listing_id' = p_listing_id::text)
                      OR (command_row.schema_version = 1
                          AND command_row.operation = 'withdraw'
                          AND command_row.listing_id = p_listing_id
                          AND command_row.result_body->>'disposition' = 'closed'
                          AND command_row.result_body->>'listing_id' = p_listing_id::text)
                  ))
              OR (p_state = 'retired' AND p_reason = 'cancelled'
                  AND (
                      (command_row.schema_version = 2
                       AND command_row.operation = 'admin_delist'
                       AND command_row.target_kind = 'listing'
                       AND command_row.target_id = p_listing_id
                       AND command_row.result_body->>'disposition' IN ('closed', 'hold_set')
                       AND command_row.result_body->>'listing_id' = p_listing_id::text)
                      OR (command_row.schema_version = 1
                          AND command_row.operation = 'admin_delist'
                          AND command_row.listing_id = p_listing_id
                          AND command_row.result_body->>'disposition' = 'closed'
                          AND command_row.result_body->>'listing_id' = p_listing_id::text)
                  ))
              OR (p_state = 'retired' AND p_reason IN ('identity_lost', 'service_disabled')
                  AND (
                      (command_row.schema_version = 2
                       AND command_row.operation = 'admin_delist'
                       AND command_row.target_kind = 'listing'
                       AND command_row.target_id = p_listing_id
                       AND command_row.result_body->>'disposition' IN ('closed', 'hold_set')
                       AND command_row.result_body->>'listing_id' = p_listing_id::text)
                      OR (command_row.schema_version = 1
                          AND command_row.operation = 'admin_delist'
                          AND command_row.listing_id = p_listing_id
                          AND command_row.result_body->>'disposition' = 'closed'
                          AND command_row.result_body->>'listing_id' = p_listing_id::text)
                  ))
              OR (p_state = 'retired' AND p_reason = 'reissued'
                  AND command_row.operation = 'issue_reservation'
                  AND command_row.target_kind = 'listing'
                  AND command_row.target_id = p_listing_id
                  AND command_row.result_body->>'disposition' = 'reservation_issued'
                  AND command_row.result_body->>'listing_id' = p_listing_id::text)
              OR (p_state = 'consumed'
                  AND ((command_row.operation = 'decide_envelope'
                        AND command_row.target_kind = 'mail_key'
                        AND command_row.result_body->>'disposition' = 'accepted')
                       OR (command_row.operation = 'release_quarantine'
                           AND command_row.target_kind = 'quarantine'
                           AND command_row.result_body->>'decision_kind' = 'accept')))
          ))
$function$;

-- 换发退休必须在同一顶层事务留下 generation+1 的 active 后继预约，且该后继正是
-- `issue_reservation` 回执里的 reservation_id。
CREATE OR REPLACE FUNCTION public.c4_reservation_successor_fact_ok(
    p_reservation_id uuid,
    p_command_id uuid
) RETURNS boolean
LANGUAGE sql SET search_path = pg_catalog
AS $function$
    SELECT EXISTS (
        SELECT 1
        FROM public.consignment_deposit_reservations AS old_reservation
        JOIN public.consignment_deposit_reservations AS successor
          ON successor.listing_id = old_reservation.listing_id
         AND successor.generation = old_reservation.generation + 1
        JOIN public.consignment_commands AS command_row
          ON command_row.command_id = p_command_id
        WHERE old_reservation.id = p_reservation_id
          AND old_reservation.state = 'retired'
          AND old_reservation.reason = 'reissued'
          AND old_reservation.xmin = pg_current_xact_id()::xid
          AND successor.state = 'active'
          AND successor.last_command_id = old_reservation.last_command_id
          AND successor.xmin = pg_current_xact_id()::xid
          AND command_row.result_body->>'reservation_id' = successor.id::text
    )
$function$;

-- 整封消费必须在同一顶层事务留下 item、终局 accepted 与对应命令事实；
-- 解除隔离的接受还必须留下同一命令的 accepted release 行。
CREATE OR REPLACE FUNCTION public.c4_reservation_consumption_fact_ok(
    p_reservation_id uuid,
    p_command_id uuid
) RETURNS boolean
LANGUAGE sql SET search_path = pg_catalog
AS $function$
    SELECT EXISTS (
        SELECT 1
        FROM public.consignment_deposit_items AS item
        JOIN public.mail_final_dispositions AS disposition
          ON disposition.id = item.decision_id
        JOIN public.consignment_commands AS command_row
          ON command_row.command_id = p_command_id
        WHERE item.reservation_id = p_reservation_id
          AND item.xmin = pg_current_xact_id()::xid
          AND disposition.kind = 'accepted'
          AND disposition.command_id = command_row.command_id
          AND disposition.xmin = pg_current_xact_id()::xid
          AND disposition.id::text = command_row.result_body->>'decision_id'
          AND (
              (command_row.operation = 'decide_envelope'
               AND command_row.result_body->>'disposition' = 'accepted')
              OR (command_row.operation = 'release_quarantine'
                  AND command_row.result_body->>'decision_kind' = 'accept'
                  AND EXISTS (
                      SELECT 1
                      FROM public.mail_quarantine_releases AS release_row
                      WHERE release_row.command_id = command_row.command_id
                        AND release_row.kind = 'accepted'
                        AND release_row.decision_id = disposition.id))
          )
    )
$function$;

CREATE OR REPLACE FUNCTION public.c4_guard_reservation() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    listing_mode text;
    listing_state text;
    listing_expires timestamptz;
    listing_seller uuid;
    season_end timestamptz;
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.state <> 'active' THEN
            RAISE EXCEPTION '预约必须从 active 开始'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_res_transition';
        END IF;
        SELECT mode, state, expires_at, seller_user_id
        INTO listing_mode, listing_state, listing_expires, listing_seller
        FROM public.consignment_listings WHERE id = NEW.listing_id;
        IF listing_mode IS DISTINCT FROM 'consigned' OR listing_state IS DISTINCT FROM 'awaiting_deposit' THEN
            RAISE EXCEPTION '只有寄售待寄挂单可以签发预约'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_res_listing_state';
        END IF;
        IF listing_seller IS DISTINCT FROM NEW.seller_user_id THEN
            RAISE EXCEPTION '预约卖家必须与挂单卖家一致'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_res_listing_seller';
        END IF;
        IF NEW.issued_at >= listing_expires OR NEW.expires_at > listing_expires THEN
            RAISE EXCEPTION '预约超悬挂单待寄截止'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_res_deadline';
        END IF;
        IF NEW.season_window_id IS NOT NULL THEN
            SELECT end_utc INTO season_end
            FROM public.consignment_season_windows WHERE id = NEW.season_window_id;
            IF season_end IS NULL OR NEW.expires_at > season_end OR NEW.issued_at >= season_end THEN
                RAISE EXCEPTION '预约超出冻结季末'
                    USING ERRCODE = '23514', CONSTRAINT = 'c4_res_deadline';
            END IF;
        END IF;
        RETURN NEW;
    END IF;

    IF OLD.state <> 'active' AND OLD IS DISTINCT FROM NEW THEN
        RAISE EXCEPTION '已退休或已消费预约不能再修改'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_res_transition';
    END IF;
    IF NEW.id <> OLD.id OR NEW.listing_id <> OLD.listing_id
       OR NEW.seller_user_id <> OLD.seller_user_id OR NEW.bot_uid <> OLD.bot_uid
       OR NEW.world_id <> OLD.world_id OR NEW.canonical_eid <> OLD.canonical_eid
       OR NEW.season_window_id IS DISTINCT FROM OLD.season_window_id
       OR NEW.generation <> OLD.generation OR NEW.service_epoch <> OLD.service_epoch
       OR NEW.session_id <> OLD.session_id OR NEW.token_digest <> OLD.token_digest
       OR NEW.issued_at <> OLD.issued_at OR NEW.expires_at <> OLD.expires_at THEN
        RAISE EXCEPTION '预约身份、代次或期限不可改写'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_res_transition';
    END IF;
    IF NEW.state NOT IN ('retired', 'consumed') THEN
        RAISE EXCEPTION '预约只能退休或消费'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_res_transition';
    END IF;
    -- 预约退休或消费是 holder 释放的前置事实，必须由本事务写入、且与退休原因和
    -- 本预约/其挂单目标相符的命令行解释；否则运行账号可用一条无关命令伪造释放
    -- （不变量 2、4、6）。命令行由 `DepositWriter::finish` 在本事务末尾写入，
    -- 因此命令绑定与换发后继、消费 item 一起放到提交前的约束触发器复核。
    IF NEW.last_command_id IS NOT DISTINCT FROM OLD.last_command_id THEN
        RAISE EXCEPTION '预约退休或消费必须引用新的命令'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_res_transition';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c4_res_transition
BEFORE INSERT OR UPDATE ON public.consignment_deposit_reservations
FOR EACH ROW EXECUTE FUNCTION public.c4_guard_reservation();

CREATE OR REPLACE FUNCTION public.c4_guard_reservation_fact() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF (NEW.state IN ('retired', 'consumed'))
       AND NOT public.c4_reservation_retirement_command_ok(
           NEW.id, NEW.listing_id, NEW.state, NEW.reason, NEW.last_command_id) THEN
        RAISE EXCEPTION '预约退休或消费必须绑定与原因和目标相符的本事务命令'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_res_transition_fact';
    END IF;
    -- 换发的后继代与 claim 换预约原子性由 c4_guard_claim_fact 的换预约分支复核；
    -- 这里只复核不依赖 claim 投影的整封消费事实。
    IF NEW.state = 'consumed' THEN
        IF NOT public.c4_reservation_consumption_fact_ok(NEW.id, NEW.last_command_id) THEN
            RAISE EXCEPTION '整封消费缺少同事务的接受项事实'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_res_transition_fact';
        END IF;
    END IF;
    RETURN NULL;
END
$function$;
CREATE CONSTRAINT TRIGGER c4_res_transition_fact
AFTER UPDATE ON public.consignment_deposit_reservations
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.c4_guard_reservation_fact();

CREATE OR REPLACE FUNCTION public.c4_guard_claim() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.revision < 1 THEN
            RAISE EXCEPTION '实物占用必须从正版本开始'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_claim_transition';
        END IF;
        RETURN NEW;
    END IF;
    IF NEW.world_id <> OLD.world_id OR NEW.canonical_eid <> OLD.canonical_eid
       OR NEW.revision <> OLD.revision + 1 OR NEW.changed_at < OLD.changed_at THEN
        RAISE EXCEPTION '实物占用身份或版本变化不合法'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_claim_transition';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c4_claim_transition
BEFORE INSERT OR UPDATE ON public.consignment_equipment_claims
FOR EACH ROW EXECUTE FUNCTION public.c4_guard_claim();

-- 实物占用投影的持有人只能由本事务写入的真实事实推进：
--   空 → 预约：同物、同卖家、本事务写入的 active 预约行；
--   空/预约 → position：本事务写入的 item 与 position，预约换 position 时该预约已消费；
--   换预约：新预约本事务写入且 active，旧预约本事务退休为 reissued；
--   预约 → 空：旧预约本事务 retired/consumed；
--   position → 空：本事务写入的 handed_over/season_ended/lost 代管事件及其 proof。
-- 所有预约的 retired/consumed 还须把 last_command_id 指向本事务写入、且与退休原因
-- 和目标相符的命令行；预约投影的触发器已强制该点，这里按不变量 2、4、6 再核对
-- 一次并复核后继预约/消费 item，避免投影被单独绕过。
-- xmin 是 32 位事务号；仓库禁止保存点，因此与 pg_current_xact_id() 的低位比较足以确认
-- 事实来自本顶层事务。任何无法由同事务事实解释的变化在 COMMIT 前失败关闭（不变量 2、6、10）。
CREATE OR REPLACE FUNCTION public.c4_guard_claim_fact() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    old_reservation uuid;
    old_position uuid;
    old_reservation_row public.consignment_deposit_reservations%ROWTYPE;
    reservation_row public.consignment_deposit_reservations%ROWTYPE;
    item_row public.consignment_deposit_items%ROWTYPE;
    position_row public.consignment_custody_positions%ROWTYPE;
    event_row public.consignment_custody_events%ROWTYPE;
BEGIN
    IF TG_OP = 'INSERT' THEN
        old_reservation := NULL;
        old_position := NULL;
    ELSE
        old_reservation := OLD.reservation_id;
        old_position := OLD.position_id;
        IF NEW.reservation_id IS NOT DISTINCT FROM OLD.reservation_id
           AND NEW.position_id IS NOT DISTINCT FROM OLD.position_id THEN
            RETURN NULL;
        END IF;
    END IF;

    IF NEW.reservation_id IS NOT NULL AND NEW.position_id IS NOT NULL THEN
        RETURN NULL;
    END IF;

    IF NEW.position_id IS NOT NULL THEN
        IF old_position IS NOT NULL THEN
            RAISE EXCEPTION '实物占用不能直接换 position'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_claim_transition_fact_ck';
        END IF;
        SELECT * INTO position_row
        FROM public.consignment_custody_positions WHERE id = NEW.position_id;
        IF position_row.id IS NULL THEN
            RAISE EXCEPTION '实物占用缺少 position 事实'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_claim_transition_fact_ck';
        END IF;
        SELECT * INTO item_row
        FROM public.consignment_deposit_items WHERE id = position_row.item_id;
        IF item_row.id IS NULL
           OR position_row.world_id <> NEW.world_id
           OR position_row.canonical_eid <> NEW.canonical_eid
           OR item_row.world_id <> NEW.world_id
           OR item_row.canonical_eid <> NEW.canonical_eid
           OR position_row.xmin <> pg_current_xact_id()::xid
           OR item_row.xmin <> pg_current_xact_id()::xid
           OR (NEW.last_fact_id <> position_row.id AND NEW.last_fact_id <> item_row.id) THEN
            RAISE EXCEPTION '实物占用缺少本事务的 item/position 事实'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_claim_transition_fact_ck';
        END IF;
        IF old_reservation IS NOT NULL THEN
            IF item_row.reservation_id <> old_reservation THEN
                RAISE EXCEPTION 'position 未消费原预约'
                    USING ERRCODE = '23514', CONSTRAINT = 'c4_claim_transition_fact_ck';
            END IF;
            SELECT * INTO old_reservation_row
            FROM public.consignment_deposit_reservations WHERE id = old_reservation;
            IF old_reservation_row.id IS NULL
               OR old_reservation_row.state <> 'consumed'
               OR old_reservation_row.xmin <> pg_current_xact_id()::xid
               OR NOT public.c4_reservation_retirement_command_ok(
                   old_reservation_row.id, old_reservation_row.listing_id,
                   old_reservation_row.state, old_reservation_row.reason,
                   old_reservation_row.last_command_id)
               OR NOT public.c4_reservation_consumption_fact_ok(
                   old_reservation_row.id, old_reservation_row.last_command_id) THEN
                RAISE EXCEPTION '预约换 position 缺少本事务的消费事实'
                    USING ERRCODE = '23514', CONSTRAINT = 'c4_claim_transition_fact_ck';
            END IF;
        END IF;
        RETURN NULL;
    END IF;

    IF NEW.reservation_id IS NOT NULL THEN
        IF old_position IS NOT NULL THEN
            RAISE EXCEPTION 'position 不能直接换成预约'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_claim_transition_fact_ck';
        END IF;
        SELECT * INTO reservation_row
        FROM public.consignment_deposit_reservations WHERE id = NEW.reservation_id;
        IF reservation_row.id IS NULL
           OR reservation_row.state <> 'active'
           OR reservation_row.world_id <> NEW.world_id
           OR reservation_row.canonical_eid <> NEW.canonical_eid
           OR reservation_row.xmin <> pg_current_xact_id()::xid
           OR NEW.last_fact_id <> reservation_row.id
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_listings AS listing
               WHERE listing.id = reservation_row.listing_id
                 AND listing.world_id = reservation_row.world_id
                 AND listing.external_equipment_id = reservation_row.canonical_eid
                 AND listing.seller_user_id = reservation_row.seller_user_id
           ) THEN
            RAISE EXCEPTION '实物占用缺少本事务的 active 预约事实'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_claim_transition_fact_ck';
        END IF;
        IF old_reservation IS NOT NULL THEN
            SELECT * INTO old_reservation_row
            FROM public.consignment_deposit_reservations WHERE id = old_reservation;
            IF old_reservation_row.id IS NULL
               OR old_reservation_row.state <> 'retired'
               OR old_reservation_row.reason <> 'reissued'
               OR old_reservation_row.xmin <> pg_current_xact_id()::xid
               OR NOT public.c4_reservation_retirement_command_ok(
                   old_reservation_row.id, old_reservation_row.listing_id,
                   old_reservation_row.state, old_reservation_row.reason,
                   old_reservation_row.last_command_id)
               OR reservation_row.generation <> old_reservation_row.generation + 1
               OR reservation_row.listing_id <> old_reservation_row.listing_id
               OR reservation_row.last_command_id <> old_reservation_row.last_command_id
               OR NOT public.c4_reservation_successor_fact_ok(
                   old_reservation_row.id, old_reservation_row.last_command_id) THEN
                RAISE EXCEPTION '换预约缺少旧预约的本事务退休事实'
                    USING ERRCODE = '23514', CONSTRAINT = 'c4_claim_transition_fact_ck';
            END IF;
        END IF;
        RETURN NULL;
    END IF;

    IF old_position IS NOT NULL THEN
        SELECT * INTO event_row
        FROM public.consignment_custody_events
        WHERE position_id = old_position
          AND event_kind IN ('handed_over', 'season_ended', 'lost')
          AND xmin = pg_current_xact_id()::xid
        ORDER BY version DESC
        LIMIT 1;
        SELECT * INTO position_row
        FROM public.consignment_custody_positions WHERE id = old_position;
        IF event_row.position_id IS NULL
           OR event_row.proof_id IS NULL
           OR NEW.last_fact_id <> event_row.proof_id
           OR position_row.id IS NULL
           OR position_row.state <> event_row.to_state
           OR position_row.xmin <> pg_current_xact_id()::xid
           OR NOT EXISTS (
               SELECT 1 FROM public.mail_disposition_proofs AS proof
               WHERE proof.id = event_row.proof_id
                 AND proof.xmin = pg_current_xact_id()::xid
           ) THEN
            RAISE EXCEPTION '释放 position 缺少本事务的终止代管事件与证明'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_claim_transition_fact_ck';
        END IF;
        RETURN NULL;
    END IF;

    IF old_reservation IS NOT NULL THEN
        SELECT * INTO old_reservation_row
        FROM public.consignment_deposit_reservations WHERE id = old_reservation;
        IF old_reservation_row.id IS NULL
           OR old_reservation_row.state NOT IN ('retired', 'consumed')
           OR old_reservation_row.reason IS NOT DISTINCT FROM 'reissued'
           OR old_reservation_row.xmin <> pg_current_xact_id()::xid
           OR NOT public.c4_reservation_retirement_command_ok(
               old_reservation_row.id, old_reservation_row.listing_id,
               old_reservation_row.state, old_reservation_row.reason,
               old_reservation_row.last_command_id)
           OR (old_reservation_row.state = 'consumed'
               AND NOT public.c4_reservation_consumption_fact_ok(
                   old_reservation_row.id, old_reservation_row.last_command_id))
           OR NEW.last_fact_id <> old_reservation_row.id THEN
            RAISE EXCEPTION '释放预约缺少本事务的 retired/consumed 事实'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_claim_transition_fact_ck';
        END IF;
    END IF;
    RETURN NULL;
END
$function$;
CREATE CONSTRAINT TRIGGER c4_claim_transition_fact
AFTER INSERT OR UPDATE OF reservation_id, position_id, last_fact_id
ON public.consignment_equipment_claims
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.c4_guard_claim_fact();

-- 资产 FK 只绑定非空的装备、世界与规范 EID；main 的 season_id 为 NULL，必须另核季，
-- 否则 MATCH SIMPLE 会让整条资产引用失效。不变量 6、10：实物身份与季界不得漂移。
CREATE OR REPLACE FUNCTION public.c4_guard_handoff_asset_season() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM public.consignment_custody_assets AS asset
        WHERE asset.equipment_id = NEW.equipment_id
          AND asset.world_id = NEW.world_id
          AND asset.canonical_eid = NEW.canonical_eid
          AND asset.season_id IS NOT DISTINCT FROM NEW.season_id
    ) THEN
        RAISE EXCEPTION '交接资产必须与原实物同季'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_handoff_edge_asset_season';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c4_handoff_edge_asset_season
BEFORE INSERT OR UPDATE OF equipment_id, world_id, canonical_eid, season_id
ON public.consignment_equipment_handoffs
FOR EACH ROW EXECUTE FUNCTION public.c4_guard_handoff_asset_season();

CREATE OR REPLACE FUNCTION public.c4_guard_position() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.state <> 'awaiting_take' OR NEW.version <> 1
           OR NEW.current_generation IS NOT NULL THEN
            RAISE EXCEPTION 'position 必须从 awaiting_take 版本 1 开始'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_position_transition';
        END IF;
        RETURN NEW;
    END IF;
    IF NEW.id <> OLD.id OR NEW.item_id <> OLD.item_id
       OR NEW.listing_id <> OLD.listing_id OR NEW.seller_user_id <> OLD.seller_user_id
       OR NEW.equipment_id <> OLD.equipment_id OR NEW.world_id <> OLD.world_id
       OR NEW.canonical_eid <> OLD.canonical_eid
       OR NEW.season_window_id IS DISTINCT FROM OLD.season_window_id
       OR NEW.version <> OLD.version + 1 OR NEW.updated_at < OLD.updated_at THEN
        RAISE EXCEPTION 'position 身份或版本变化不合法'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_position_transition';
    END IF;
    IF OLD.state = 'awaiting_take' AND NEW.state = 'in_custody' THEN
        IF NEW.current_generation IS DISTINCT FROM 1 THEN
            RAISE EXCEPTION '首次取得必须创建处分代 1'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_position_transition';
        END IF;
    ELSIF OLD.state = 'in_custody' AND NEW.state = 'reconcile' THEN
        IF NEW.current_generation IS DISTINCT FROM OLD.current_generation THEN
            RAISE EXCEPTION '核对不能替换当前处分代'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_position_transition';
        END IF;
    ELSIF OLD.state = 'reconcile' AND NEW.state = 'in_custody' THEN
        IF NEW.current_generation IS DISTINCT FROM OLD.current_generation THEN
            RAISE EXCEPTION '核对返回不能替换当前处分代'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_position_transition';
        END IF;
    ELSIF NEW.state IN ('committed', 'in_transit', 'handed_over', 'season_ended', 'lost') THEN
        RAISE EXCEPTION '第 4 期不能产生该 position 迁移'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_position_phase4_transition';
    ELSE
        RAISE EXCEPTION 'position 迁移不合法'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_position_transition';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c4_position_transition
BEFORE INSERT OR UPDATE ON public.consignment_custody_positions
FOR EACH ROW EXECUTE FUNCTION public.c4_guard_position();

CREATE OR REPLACE FUNCTION public.c4_guard_control() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    expected_version bigint;
    predecessor_kind text;
    predecessor_position uuid;
BEGIN
    SELECT COALESCE(max(version), 0) + 1 INTO expected_version
    FROM public.consignment_custody_controls WHERE position_id = NEW.position_id;
    IF NEW.version <> expected_version THEN
        RAISE EXCEPTION '代管控制版本必须连续'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_control_transition';
    END IF;
    IF NEW.kind IN ('withdrawal_intent', 'hold_set') AND NEW.predecessor_id IS NOT NULL THEN
        RAISE EXCEPTION '意愿或设止售不能有前驱'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_control_predecessor';
    END IF;
    IF NEW.kind = 'hold_released' THEN
        SELECT kind, position_id INTO predecessor_kind, predecessor_position
        FROM public.consignment_custody_controls WHERE id = NEW.predecessor_id;
        IF predecessor_kind IS DISTINCT FROM 'hold_set'
           OR predecessor_position IS DISTINCT FROM NEW.position_id THEN
            RAISE EXCEPTION '解除止售必须引用同 position 的设止售'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_control_predecessor';
        END IF;
    END IF;
    IF NEW.kind = 'hold_set' AND EXISTS (
        SELECT 1 FROM public.consignment_custody_controls AS hold
        WHERE hold.position_id = NEW.position_id AND hold.kind = 'hold_set'
          AND NOT EXISTS (
              SELECT 1 FROM public.consignment_custody_controls AS release
              WHERE release.predecessor_id = hold.id)
    ) THEN
        RAISE EXCEPTION '同一 position 只能有一个未解除止售'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_control_hold_once';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c4_control_transition
BEFORE INSERT ON public.consignment_custody_controls
FOR EACH ROW EXECUTE FUNCTION public.c4_guard_control();

CREATE OR REPLACE FUNCTION public.c4_guard_generation() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NEW.generation = 1 THEN
        IF NEW.predecessor IS NOT NULL THEN
            RAISE EXCEPTION '首次处分代不能有前驱'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_generation_transition';
        END IF;
    ELSIF NOT EXISTS (
        SELECT 1 FROM public.consignment_disposition_generations
        WHERE position_id = NEW.position_id AND generation = NEW.predecessor
    ) THEN
        RAISE EXCEPTION '处分代前驱必须存在'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_generation_transition';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c4_generation_transition
BEFORE INSERT ON public.consignment_disposition_generations
FOR EACH ROW EXECUTE FUNCTION public.c4_guard_generation();

CREATE OR REPLACE FUNCTION public.c4_guard_asset_alias() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    alias_world text;
    alias_eid text;
BEGIN
    IF NEW.auction_equipment_id IS NOT NULL THEN
        SELECT world_id, public.consignment_eid_alias(external_equipment_id)
        INTO alias_world, alias_eid
        FROM public.equipment WHERE id = NEW.auction_equipment_id;
        IF alias_world IS DISTINCT FROM NEW.world_id
           OR alias_eid IS DISTINCT FROM NEW.canonical_eid
           OR NOT EXISTS (
               SELECT 1 FROM public.lots WHERE equipment_id = NEW.auction_equipment_id) THEN
            RAISE EXCEPTION '寄售实物与旧拍卖别名不一致'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_asset_alias';
        END IF;
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c4_asset_alias
BEFORE INSERT OR UPDATE OF auction_equipment_id ON public.consignment_custody_assets
FOR EACH ROW EXECUTE FUNCTION public.c4_guard_asset_alias();

CREATE OR REPLACE FUNCTION public.c4_guard_recovery_handoff() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM public.settlement_recoveries AS recovery
        WHERE recovery.id = NEW.recovery_id
          AND NEW.obligation_id IN (recovery.payment_obligation_id,
                                    recovery.equipment_obligation_id)
    ) THEN
        RAISE EXCEPTION '恢复接管义务必须属于原 recovery 的付款或交割义务'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_recovery_handoff_obligation';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c4_recovery_handoff_obligation
BEFORE INSERT ON public.settlement_recovery_return_handoffs
FOR EACH ROW EXECUTE FUNCTION public.c4_guard_recovery_handoff();

CREATE OR REPLACE FUNCTION public.c4_guard_manifest_conflict() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF EXISTS (
        SELECT 1 FROM public.mail_envelope_observations
        WHERE id = NEW.observation_id AND manifest_digest = NEW.conflicting_digest
    ) THEN
        RAISE EXCEPTION '观察冲突摘要不能与原摘要相同'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_conflict_distinct';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c4_conflict_distinct
BEFORE INSERT ON public.mail_observation_conflicts
FOR EACH ROW EXECUTE FUNCTION public.c4_guard_manifest_conflict();

CREATE OR REPLACE FUNCTION public.c4_reject_acquire_return_without_intake() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NEW.kind = 'acquire_return' THEN
        -- 第 4 期没有寄售原轮 intake 写端；协议值保留，但任何成功行都必须失败关闭。
        RAISE EXCEPTION '第 4 期不能产生 acquire_return 成功行'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_acquire_return_intake_required';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c4_final_acquire_return_intake
BEFORE INSERT OR UPDATE OF kind ON public.mail_final_dispositions
FOR EACH ROW EXECUTE FUNCTION public.c4_reject_acquire_return_without_intake();
CREATE TRIGGER c4_release_acquire_return_intake
BEFORE INSERT OR UPDATE OF kind ON public.mail_quarantine_releases
FOR EACH ROW EXECUTE FUNCTION public.c4_reject_acquire_return_without_intake();

CREATE OR REPLACE FUNCTION public.c4_guard_final_decision() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    initial_kind text;
    initial_observation uuid;
BEGIN
    SELECT kind, observation_id INTO initial_kind, initial_observation
    FROM public.mail_initial_decisions
    WHERE bot_uid = NEW.bot_uid AND world_id = NEW.world_id
      AND external_mid = NEW.external_mid;
    IF initial_kind IS NULL THEN
        RAISE EXCEPTION '最终处置缺少同 K 首次决定'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_final_initial';
    END IF;
    IF initial_kind <> 'quarantined' AND NEW.kind <> initial_kind::text THEN
        RAISE EXCEPTION '最终处置种类与首次决定不兼容'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_final_initial';
    END IF;
    IF initial_kind <> 'quarantined' AND NEW.source_observation_id <> initial_observation THEN
        RAISE EXCEPTION '最终处置必须引用首次决定的原观察'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_final_observation';
    END IF;
    IF initial_kind = 'quarantined'
       AND NEW.kind IN ('accepted', 'return_decided', 'archived_empty') THEN
        RETURN NEW;
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c4_final_initial
BEFORE INSERT ON public.mail_final_dispositions
FOR EACH ROW EXECUTE FUNCTION public.c4_guard_final_decision();

-- 提交前整封校验。约束触发器在 COMMIT 时执行；任何条数、身份、版本、占用或唯一 Take 不符
-- 都让整个顶层事务回滚。
CREATE OR REPLACE FUNCTION public.c4_check_final_disposition() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    observed_raw_count integer;
    item_count integer;
    distinct_eids integer;
    min_ordinal integer;
    max_ordinal integer;
    position_count integer;
    reservation_count integer;
    take_count integer;
    return_count integer;
    release_count integer;
BEGIN
    IF NEW.kind = 'accepted' THEN
        SELECT observation.raw_count INTO observed_raw_count
        FROM public.mail_envelope_observations AS observation
        WHERE observation.id = NEW.source_observation_id;
        SELECT count(*), count(DISTINCT canonical_eid), min(ordinal), max(ordinal)
        INTO item_count, distinct_eids, min_ordinal, max_ordinal
        FROM public.consignment_deposit_items WHERE decision_id = NEW.id;
        SELECT count(*) INTO position_count
        FROM public.consignment_custody_positions AS position
        JOIN public.consignment_deposit_items AS item ON item.id = position.item_id
        WHERE item.decision_id = NEW.id;
        SELECT count(DISTINCT reservation_id) INTO reservation_count
        FROM public.consignment_deposit_items WHERE decision_id = NEW.id;
        SELECT count(*) FILTER (WHERE action = 'take'),
               count(*) FILTER (WHERE action = 'return')
        INTO take_count, return_count
        FROM public.mail_disposition_work WHERE decision_id = NEW.id;

        IF observed_raw_count IS NULL
           OR item_count < 1 OR item_count > 20
           OR observed_raw_count <> item_count
           OR distinct_eids <> item_count
           OR position_count <> item_count
           OR reservation_count <> item_count
           OR min_ordinal <> 1 OR max_ordinal <> item_count
           OR take_count <> 1 OR return_count <> 0 THEN
            RAISE EXCEPTION '整封接受条数、预约、position 或唯一 Take 不一致'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_envelope_accept_counts';
        END IF;

        IF EXISTS (
            SELECT 1
            FROM public.consignment_deposit_items AS item
            JOIN public.consignment_listings AS listing ON listing.id = item.listing_id
            JOIN public.consignment_deposit_reservations AS reservation
              ON reservation.id = item.reservation_id
            LEFT JOIN public.consignment_custody_positions AS position
              ON position.item_id = item.id
            LEFT JOIN public.consignment_equipment_claims AS claim
              ON claim.world_id = item.world_id AND claim.canonical_eid = item.canonical_eid
            WHERE item.decision_id = NEW.id
              AND (
                  listing.state <> 'deposit_accepted'
                  OR listing.state_version <> item.accepted_listing_version
                  OR listing.price <> item.accepted_price
                  -- 挂单卖家已由 c4_item_listing_fk 的复合外键绑定；这里保留纵深防御，
                  -- 正常路径不可单独触发（只有绕过外键的修复或恢复路径才会到达）。
                  OR listing.seller_user_id <> item.seller_user_id
                  OR reservation.state <> 'consumed'
                  OR reservation.seller_user_id <> item.seller_user_id
                  OR reservation.listing_id <> item.listing_id
                  OR reservation.world_id <> item.world_id
                  OR reservation.canonical_eid <> item.canonical_eid
                  OR position.id IS NULL
                  OR position.state <> 'awaiting_take'
                  OR position.listing_id <> item.listing_id
                  OR position.seller_user_id <> item.seller_user_id
                  OR position.equipment_id <> item.equipment_id
                  OR position.world_id <> item.world_id
                  OR position.canonical_eid <> item.canonical_eid
                  OR claim.position_id IS DISTINCT FROM position.id
                  OR NOT EXISTS (
                      SELECT 1 FROM public.consignment_events AS event
                      WHERE event.listing_id = item.listing_id
                        AND event.state_version = item.accepted_listing_version
                        AND event.event_kind = 'deposit_accepted'
                        -- 最终决定与它的接受事件必须来自同一条命令；actor 只限定闭集，
                        -- 允许 Admin/自动解除在同一命令里写 accept。
                        AND event.command_id = NEW.command_id
                        AND event.actor_kind IN ('worker', 'admin', 'system')
                  )
              )
        ) THEN
            RAISE EXCEPTION '整封接受项缺少同卖家、版本事件、强占用或 position'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_envelope_accept_bindings';
        END IF;
        RETURN NULL;
    END IF;

    SELECT count(*) INTO item_count
    FROM public.consignment_deposit_items WHERE decision_id = NEW.id;
    SELECT count(*) FILTER (WHERE action = 'take'),
           count(*) FILTER (WHERE action = 'return')
    INTO take_count, return_count
    FROM public.mail_disposition_work WHERE decision_id = NEW.id;

    IF NEW.kind = 'return_decided' THEN
        IF item_count <> 0 OR take_count <> 0 OR return_count <> 1 THEN
            RAISE EXCEPTION '整封拒退必须恰有一个 Return 且零 item/Take'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_envelope_return_shape';
        END IF;
        RETURN NULL;
    ELSIF NEW.kind = 'archived_empty' THEN
        IF item_count <> 0 OR take_count <> 0 OR return_count <> 0 THEN
            RAISE EXCEPTION '空信归档必须零工作'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_envelope_empty_shape';
        END IF;
        RETURN NULL;
    ELSIF NEW.kind IN ('auction_return_case', 'auction_recovery_return') THEN
        SELECT count(*) INTO release_count
        FROM public.mail_quarantine_releases
        WHERE decision_id = NEW.id AND kind = NEW.kind;
        IF release_count <> 1 OR item_count <> 0 OR take_count <> 0 OR return_count <> 0 THEN
            RAISE EXCEPTION '旧 owner 交接必须恰一个相容 owner 且零寄售资产工作'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_owner_handoff_shape';
        END IF;
        IF NEW.kind = 'auction_return_case' AND NOT EXISTS (
            SELECT 1 FROM public.mail_quarantine_releases
            WHERE decision_id = NEW.id AND kind = NEW.kind
              AND case_return_mail_id IS NOT NULL
              AND case_evidence_id = case_return_mail_id
              AND recovery_handoff_id IS NULL
        ) THEN
            RAISE EXCEPTION '拍卖退件案件交接缺少原案件证据'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_owner_handoff_shape';
        END IF;
        IF NEW.kind = 'auction_recovery_return' AND NOT EXISTS (
            SELECT 1 FROM public.mail_quarantine_releases
            WHERE decision_id = NEW.id AND kind = NEW.kind
              AND recovery_handoff_id IS NOT NULL
              AND case_return_mail_id IS NULL AND case_evidence_id IS NULL
        ) THEN
            RAISE EXCEPTION '拍卖恢复交接缺少唯一恢复接管引用'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_owner_handoff_shape';
        END IF;
        RETURN NULL;
    ELSIF NEW.kind = 'acquire_return' THEN
        RAISE EXCEPTION '第 4 期不能产生 acquire_return 成功行'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_acquire_return_intake_required';
    END IF;
    RETURN NULL;
END
$function$;

CREATE CONSTRAINT TRIGGER c4_final_disposition_envelope_guard
AFTER INSERT OR UPDATE ON public.mail_final_dispositions
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.c4_check_final_disposition();

CREATE OR REPLACE FUNCTION public.c4_check_initial_final() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NEW.kind = 'quarantined' THEN
        IF NOT EXISTS (
            SELECT 1 FROM public.mail_quarantines
            WHERE initial_decision_id = NEW.id AND bot_uid = NEW.bot_uid
              AND world_id = NEW.world_id AND external_mid = NEW.external_mid
        ) THEN
            RAISE EXCEPTION '首次隔离决定缺少隔离事实'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_initial_quarantine_presence';
        END IF;
    ELSIF NOT EXISTS (
        SELECT 1 FROM public.mail_final_dispositions
        WHERE bot_uid = NEW.bot_uid AND world_id = NEW.world_id
          AND external_mid = NEW.external_mid AND kind = NEW.kind
    ) THEN
        RAISE EXCEPTION '首次最终决定缺少同 K 最终处置'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_initial_final_presence';
    END IF;
    RETURN NULL;
END
$function$;
CREATE CONSTRAINT TRIGGER c4_initial_final_presence
AFTER INSERT ON public.mail_initial_decisions
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.c4_check_initial_final();

CREATE OR REPLACE FUNCTION public.c4_check_same_decision_xact() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM public.mail_final_dispositions
        WHERE id = NEW.decision_id AND xmin = pg_current_xact_id()::xid
    ) THEN
        RAISE EXCEPTION '整封事实必须与最终处置在同一顶层事务写入'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_same_decision_xact';
    END IF;
    RETURN NULL;
END
$function$;
CREATE CONSTRAINT TRIGGER c4_item_same_decision_xact
AFTER INSERT OR UPDATE ON public.consignment_deposit_items
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.c4_check_same_decision_xact();
CREATE CONSTRAINT TRIGGER c4_work_same_decision_xact
AFTER INSERT OR UPDATE ON public.mail_disposition_work
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW
WHEN (NEW.decision_id IS NOT NULL)
EXECUTE FUNCTION public.c4_check_same_decision_xact();

-- 尝试必须绑定原工作投递与原 K/凭据会话；证明必须绑定同 K 工作、终局与原观察。
CREATE OR REPLACE FUNCTION public.c4_guard_disposition_attempt() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    work_delivery uuid;
    work_bot text;
    work_world text;
    session_bot text;
    session_world text;
    session_credential uuid;
    session_credential_version bigint;
    session_runtime_generation bigint;
BEGIN
    SELECT delivery_id, bot_uid, world_id
    INTO work_delivery, work_bot, work_world
    FROM public.mail_disposition_work WHERE id = NEW.work_id;
    IF work_delivery IS DISTINCT FROM NEW.delivery_id THEN
        RAISE EXCEPTION '投递尝试必须使用原工作的 delivery'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_attempt_delivery';
    END IF;
    SELECT bot_uid, world_id, credential_id, credential_version, runtime_generation
    INTO session_bot, session_world, session_credential, session_credential_version,
         session_runtime_generation
    FROM public.mail_evidence_sessions WHERE id = NEW.session_id;
    IF work_bot IS NOT NULL
       AND (session_bot IS DISTINCT FROM work_bot
            OR session_world IS DISTINCT FROM work_world) THEN
        RAISE EXCEPTION '投递尝试会话必须匹配原工作的 K'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_attempt_session';
    END IF;
    IF session_credential IS DISTINCT FROM NEW.credential_id
       OR session_credential_version IS DISTINCT FROM NEW.credential_version
       OR session_runtime_generation IS DISTINCT FROM NEW.runtime_generation THEN
        RAISE EXCEPTION '投递尝试必须绑定原会话的凭据代次'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_attempt_session';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c4_attempt_binding
BEFORE INSERT ON public.mail_disposition_attempts
FOR EACH ROW EXECUTE FUNCTION public.c4_guard_disposition_attempt();

CREATE OR REPLACE FUNCTION public.c4_guard_disposition_proof() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    work_bot text;
    work_world text;
    work_mid text;
    observation_bot text;
    observation_world text;
    observation_mid text;
    decision_bot text;
    decision_world text;
    decision_mid text;
BEGIN
    SELECT bot_uid, world_id, external_mid
    INTO work_bot, work_world, work_mid
    FROM public.mail_disposition_work WHERE id = NEW.work_id;
    IF work_bot IS NULL OR work_world IS NULL OR work_mid IS NULL THEN
        RAISE EXCEPTION '处分证明必须绑定带 K 的资产工作'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_proof_work_k';
    END IF;
    SELECT bot_uid, world_id, external_mid
    INTO observation_bot, observation_world, observation_mid
    FROM public.mail_envelope_observations WHERE id = NEW.observation_id;
    IF observation_bot IS DISTINCT FROM work_bot
       OR observation_world IS DISTINCT FROM work_world
       OR observation_mid IS DISTINCT FROM work_mid THEN
        RAISE EXCEPTION '处分证明观察必须与原工作同 K'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_proof_observation_k';
    END IF;
    IF NEW.decision_id IS NOT NULL THEN
        SELECT bot_uid, world_id, external_mid
        INTO decision_bot, decision_world, decision_mid
        FROM public.mail_final_dispositions WHERE id = NEW.decision_id;
        IF decision_bot IS DISTINCT FROM work_bot
           OR decision_world IS DISTINCT FROM work_world
           OR decision_mid IS DISTINCT FROM work_mid THEN
            RAISE EXCEPTION '处分证明终局必须与原工作同 K'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_proof_decision_k';
        END IF;
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c4_proof_binding
BEFORE INSERT ON public.mail_disposition_proofs
FOR EACH ROW EXECUTE FUNCTION public.c4_guard_disposition_proof();

-- 信封退休守卫与 000132 复用同一函数；投射表只开放合同列。
CREATE TRIGGER c4_res_secret_retirement
BEFORE UPDATE ON public.consignment_reservation_secrets
FOR EACH ROW EXECUTE FUNCTION public.consignment_guard_retired_envelope();
CREATE TRIGGER c4_evidence_envelope_retirement
BEFORE UPDATE ON public.mail_evidence_envelopes
FOR EACH ROW EXECUTE FUNCTION public.consignment_guard_retired_envelope();

-- 拍卖 lot 与寄售强占用共用同一物实行。插入/变更装备身份时条件写同一行：
-- 已强占用则拒绝；空闲行推进 revision。Serializable 或唯一冲突由调用方按 40001/40P01 重试。
-- 不变量 2、6、10：不得绕过同一 (world, canonical_eid) 的互斥行。
CREATE OR REPLACE FUNCTION public.c4_guard_lot_equipment() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    equipment_world text;
    equipment_eid text;
    claimed text;
BEGIN
    IF TG_OP = 'UPDATE' AND NEW.equipment_id = OLD.equipment_id THEN
        RETURN NEW;
    END IF;
    SELECT world_id, public.consignment_eid_alias(external_equipment_id)
    INTO equipment_world, equipment_eid
    FROM public.equipment WHERE id = NEW.equipment_id;
    IF equipment_eid IS NULL THEN
        RETURN NEW;
    END IF;
    INSERT INTO public.consignment_equipment_claims
        (world_id, canonical_eid, revision, reservation_id, position_id, changed_at, last_fact_id)
    VALUES (equipment_world, equipment_eid, 1, NULL, NULL, transaction_timestamp(), NEW.id)
    ON CONFLICT (world_id, canonical_eid) DO UPDATE
    SET revision = public.consignment_equipment_claims.revision + 1,
        changed_at = excluded.changed_at,
        last_fact_id = excluded.last_fact_id
    WHERE public.consignment_equipment_claims.reservation_id IS NULL
      AND public.consignment_equipment_claims.position_id IS NULL
    RETURNING public.consignment_equipment_claims.world_id INTO claimed;
    IF claimed IS NULL THEN
        RAISE EXCEPTION '该实物已被寄售强占用'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_claims_equipment_busy';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c4_lots_equipment_guard
BEFORE INSERT OR UPDATE OF equipment_id ON public.lots
FOR EACH ROW EXECUTE FUNCTION public.c4_guard_lot_equipment();

-- 运行账号授权入口：继承 000132 的全部守卫，再收紧并重授本期精确矩阵。
ALTER FUNCTION public.apply_auction_runtime_privileges()
    RENAME TO apply_auction_runtime_privileges_000132;

CREATE FUNCTION public.apply_auction_runtime_privileges()
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $function$
DECLARE
    function_owner oid;
    grantee record;
    table_name text;
BEGIN
    SELECT procedure.proowner INTO function_owner
    FROM pg_catalog.pg_proc AS procedure
    WHERE procedure.oid =
        'public.apply_auction_runtime_privileges()'::pg_catalog.regprocedure;
    IF function_owner IS NULL OR function_owner <> current_user::regrole::oid THEN
        RAISE EXCEPTION '运行时权限入口必须由当前迁移 owner 拥有'
            USING ERRCODE = '42501';
    END IF;

    -- pg_restore --no-privileges 会把函数 ACL 还原为默认 PUBLIC EXECUTE；每次入口执行都重新收口。
    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM PUBLIC;
    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000132() FROM PUBLIC;
    FOR grantee IN
        SELECT role.rolname FROM pg_catalog.pg_roles AS role WHERE role.oid <> function_owner
    LOOP
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM %I',
            grantee.rolname
        );
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000132() FROM %I',
            grantee.rolname
        );
    END LOOP;

    PERFORM public.apply_auction_runtime_privileges_000132();

    FOREACH table_name IN ARRAY ARRAY[
        'consignment_season_windows',
        'mail_evidence_sessions',
        'consignment_mail_readiness',
        'mail_originals',
        'mail_original_links',
        'mail_evidence_envelopes',
        'mail_envelope_observations',
        'mail_observation_conflicts',
        'mail_initial_decisions',
        'mail_quarantines',
        'mail_quarantine_rechecks',
        'mail_final_dispositions',
        'mail_quarantine_releases',
        'settlement_recovery_return_handoffs',
        'consignment_deposit_reservations',
        'consignment_reservation_secrets',
        'consignment_equipment_claims',
        'consignment_custody_assets',
        'consignment_deposit_items',
        'consignment_custody_positions',
        'consignment_disposition_generations',
        'consignment_custody_events',
        'consignment_custody_controls',
        'consignment_equipment_handoffs',
        'consignment_season_notices',
        'mail_disposition_work',
        'mail_disposition_attempts',
        'mail_disposition_write_starts',
        'mail_disposition_proofs'
    ] LOOP
        EXECUTE format('REVOKE ALL ON TABLE public.%I FROM auction_runtime', table_name);
    END LOOP;

    GRANT SELECT, INSERT ON TABLE
        public.consignment_season_windows,
        public.mail_evidence_sessions,
        public.consignment_mail_readiness,
        public.mail_originals,
        public.mail_original_links,
        public.mail_evidence_envelopes,
        public.mail_envelope_observations,
        public.mail_observation_conflicts,
        public.mail_initial_decisions,
        public.mail_quarantines,
        public.mail_quarantine_rechecks,
        public.mail_final_dispositions,
        public.mail_quarantine_releases,
        public.settlement_recovery_return_handoffs,
        public.consignment_deposit_reservations,
        public.consignment_reservation_secrets,
        public.consignment_equipment_claims,
        public.consignment_custody_assets,
        public.consignment_deposit_items,
        public.consignment_custody_positions,
        public.consignment_disposition_generations,
        public.consignment_custody_events,
        public.consignment_custody_controls,
        public.consignment_equipment_handoffs,
        public.consignment_season_notices,
        public.mail_disposition_work,
        public.mail_disposition_attempts,
        public.mail_disposition_write_starts,
        public.mail_disposition_proofs
    TO auction_runtime;

    GRANT UPDATE (session_id, service_epoch, policy_digest, allowlist_mode, allowed_senders,
                  validator_versions, detail_ready, take_ready, return_ready, checked_at,
                  valid_until, version)
        ON public.consignment_mail_readiness TO auction_runtime;
    GRANT UPDATE (ciphertext, nonce, encryption_key_id, fingerprint, updated_at, retired_at)
        ON public.mail_evidence_envelopes TO auction_runtime;
    GRANT UPDATE (version, state, updated_at)
        ON public.mail_quarantines TO auction_runtime;
    GRANT UPDATE (state, available_at, lease_token, lease_until, fence, attempt_count, completed_at)
        ON public.mail_quarantine_rechecks TO auction_runtime;
    GRANT UPDATE (state, last_command_id, retired_at, reason)
        ON public.consignment_deposit_reservations TO auction_runtime;
    GRANT UPDATE (ciphertext, nonce, encryption_key_id, fingerprint, updated_at, retired_at)
        ON public.consignment_reservation_secrets TO auction_runtime;
    GRANT UPDATE (revision, reservation_id, position_id, changed_at, last_fact_id)
        ON public.consignment_equipment_claims TO auction_runtime;
    GRANT UPDATE (state, version, current_generation, updated_at)
        ON public.consignment_custody_positions TO auction_runtime;

    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM PUBLIC;
    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000132() FROM PUBLIC;
    FOR grantee IN
        SELECT role.rolname FROM pg_catalog.pg_roles AS role WHERE role.oid <> function_owner
    LOOP
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM %I',
            grantee.rolname
        );
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000132() FROM %I',
            grantee.rolname
        );
    END LOOP;
END
$function$;

REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM PUBLIC, auction_runtime;
SELECT public.apply_auction_runtime_privileges();

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (134, 'breaking');
