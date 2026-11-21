-- 资金分录只引用同 UID 对象，发行不超过冻结额度；历史 seller claim 与冲正绑定唯一成交。
-- owner: auction-storage
-- Breaking：新增并发约束并拒绝无法证明的既有关系；升级前须停止 Worker。
LOCK TABLE
    public.auction_settlements,
    public.auctions,
    public.compensation_pool_reversals,
    public.lot_settlements,
    public.lots,
    public.settlement_funding_accounts,
    public.settlement_funding_allocations,
    public.settlement_funding_entries,
    public.settlement_funding_lots,
    public.settlement_funding_sources,
    public.settlement_funding_receipts,
    public.settlement_equipment_losses,
    public.settlement_legacy_bot_seller_claims,
    public.settlement_obligations,
    public.settlement_recoveries
IN SHARE ROW EXCLUSIVE MODE;
CREATE FUNCTION public.settlement_record_only_source_valid_000120(
    checked_source_id uuid,
    checked_credit_id uuid,
    require_current_state boolean
)
RETURNS boolean
LANGUAGE sql
STABLE
SET search_path = pg_catalog, public
AS $function$
    SELECT EXISTS (
        SELECT 1
        FROM public.settlement_funding_sources AS source
        JOIN public.settlement_funding_accounts AS account ON account.uid = source.uid
        JOIN public.settlement_funding_lots AS funded_lot
          ON funded_lot.uid = source.uid
         AND funded_lot.payment_obligation_id = source.source_key
        JOIN public.settlement_obligations AS payment
          ON payment.id = funded_lot.payment_obligation_id
         AND payment.lot_id = funded_lot.lot_id
        JOIN public.settlement_funding_allocations AS seller_allocation
          ON seller_allocation.uid = source.uid
         AND seller_allocation.purpose = 'seller'
         AND seller_allocation.lot_id = funded_lot.lot_id
        JOIN public.settlement_obligations AS seller
          ON seller.id = seller_allocation.obligation_id
         AND seller.lot_id = funded_lot.lot_id
         AND seller.obligation_kind = 'seller_payout'
         AND (
             seller.prerequisite_obligation_id = payment.id
             OR EXISTS (
                 SELECT 1 FROM public.settlement_obligations AS handoff
                 WHERE handoff.id = seller.prerequisite_obligation_id
                   AND handoff.lot_id = funded_lot.lot_id
                   AND handoff.obligation_kind = 'winner_equipment_handoff'
                   AND handoff.prerequisite_obligation_id = payment.id
             )
         )
        JOIN public.lot_settlements AS settlement ON settlement.lot_id = funded_lot.lot_id
        JOIN public.lots AS lot
          ON lot.id = funded_lot.lot_id
         AND lot.auction_id = payment.auction_id
         AND lot.equipment_id = payment.equipment_id
        JOIN public.auctions AS auction ON auction.id = lot.auction_id
        WHERE source.id = checked_source_id
          AND account.accounting_mode = 'record_only'
          AND account.status = 'ready'
          AND auction.status = 'settled'
          AND lot.status = 'sold'
          AND payment.obligation_kind IN ('winner_equipment_cod', 'winner_payment_carrier_cod')
          AND payment.amount = settlement.buyer_amount
          AND seller.amount = settlement.seller_amount
          AND settlement.buyer_amount::numeric + settlement.compensation_amount::numeric
              = settlement.seller_amount::numeric + settlement.tax_amount::numeric
          AND (NOT require_current_state OR funded_lot.disposition = 'seller')
          AND (
              (require_current_state
               AND checked_credit_id = seller_allocation.id
               AND seller_allocation.state = 'available')
              OR (
                  NOT require_current_state
                  AND checked_credit_id IS NULL
                  AND NOT EXISTS (
                      SELECT 1 FROM public.settlement_funding_entries AS entry
                      WHERE entry.source_id = source.id
                        AND entry.kind = 'issue'
                        AND entry.credit_id IS DISTINCT FROM seller_allocation.id
                  )
              )
          )
          AND (
              (source.kind = 'record_internal_purchase'
               AND funded_lot.source_kind = 'internal'
               AND source.amount = settlement.buyer_amount)
              OR (
                  source.kind = 'record_operator_equipment_loss'
                  AND funded_lot.source_kind = 'operator_equipment_loss'
                  AND payment.obligation_kind = 'winner_equipment_cod'
                  AND source.amount = settlement.buyer_amount
                  AND EXISTS (
                      SELECT 1 FROM public.settlement_equipment_losses AS loss
                      WHERE loss.lot_id = funded_lot.lot_id
                        AND loss.payment_obligation_id = payment.id
                        AND loss.uid = source.uid
                        AND loss.buyer_amount = settlement.buyer_amount
                        AND loss.compensation_amount = settlement.compensation_amount
                        AND loss.seller_amount = settlement.seller_amount
                        AND loss.tax_amount = settlement.tax_amount
                        AND loss.request_digest = source.proof_digest
                  )
              )
              OR (
                  source.kind = 'record_compensation'
                  AND source.amount = settlement.compensation_amount
                  AND (
                      (funded_lot.source_kind = 'internal')
                      OR (
                          funded_lot.source_kind = 'operator_equipment_loss'
                          AND EXISTS (
                              SELECT 1 FROM public.settlement_equipment_losses AS loss
                              WHERE loss.lot_id = funded_lot.lot_id
                                AND loss.payment_obligation_id = payment.id
                                AND loss.uid = source.uid
                                AND loss.buyer_amount = settlement.buyer_amount
                                AND loss.compensation_amount = settlement.compensation_amount
                                AND loss.seller_amount = settlement.seller_amount
                                AND loss.tax_amount = settlement.tax_amount
                                AND loss.request_digest = source.proof_digest
                          )
                      )
                      OR (
                          funded_lot.source_kind = 'collected'
                          AND EXISTS (
                              SELECT 1
                              FROM public.settlement_funding_receipts AS receipt
                              JOIN public.settlement_funding_sources AS buyer_source
                                ON buyer_source.id = receipt.source_id
                              WHERE receipt.uid = source.uid
                                AND receipt.payment_obligation_id = payment.id
                                AND buyer_source.uid = source.uid
                                AND buyer_source.kind = 'collected_payment'
                                AND buyer_source.source_key = payment.id
                                AND buyer_source.amount = settlement.buyer_amount
                          )
                      )
                  )
              )
          )
          AND (
              require_current_state
              OR NOT EXISTS (
                  SELECT 1 FROM public.settlement_funding_entries AS entry
                  WHERE entry.source_id = source.id
                    AND entry.kind = 'issue'
                    AND entry.proof_digest IS DISTINCT FROM source.proof_digest
              )
          )
    )
$function$;

-- 000070 的独立外键不保证冲正与 recovery/场次/lot 结算事实属于同一笔成交。
DO $block$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM public.compensation_pool_reversals AS reversal
        LEFT JOIN public.settlement_recoveries AS recovery
          ON recovery.id = reversal.recovery_id
        LEFT JOIN public.auctions AS auction ON auction.id = reversal.auction_id
        LEFT JOIN public.auction_settlements AS auction_settlement
          ON auction_settlement.auction_id = reversal.auction_id
        LEFT JOIN public.lots AS lot
          ON lot.id = reversal.lot_id AND lot.auction_id = reversal.auction_id
        LEFT JOIN public.lot_settlements AS lot_settlement
          ON lot_settlement.lot_id = reversal.lot_id
        WHERE recovery.id IS NULL OR auction.id IS NULL OR auction_settlement.auction_id IS NULL
           OR lot.id IS NULL OR lot_settlement.lot_id IS NULL
           OR recovery.auction_id IS DISTINCT FROM reversal.auction_id
           OR recovery.lot_id IS DISTINCT FROM reversal.lot_id
           OR recovery.world_id IS DISTINCT FROM auction.world_id
           OR recovery.resolution IS DISTINCT FROM reversal.resolution
           OR lot_settlement.tax_amount IS DISTINCT FROM reversal.tax_amount
           OR lot_settlement.compensation_amount IS DISTINCT FROM reversal.compensation_amount
    ) THEN
        RAISE EXCEPTION '既有补偿池冲正与 recovery/结算事实不一致；须人工核对'
            USING ERRCODE = '23514';
    END IF;
END
$block$;

CREATE FUNCTION public.validate_compensation_reversal_000120()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
DECLARE
    checked_recovery_id uuid;
BEGIN
    IF TG_TABLE_NAME = 'compensation_pool_reversals' THEN
        IF TG_OP = 'DELETE' THEN
            checked_recovery_id := OLD.recovery_id;
        ELSE
            checked_recovery_id := NEW.recovery_id;
        END IF;
    ELSE
        checked_recovery_id := NEW.id;
    END IF;
    PERFORM pg_catalog.pg_advisory_xact_lock(
        pg_catalog.hashtextextended('compensation-reversal:' || checked_recovery_id::text, 0)
    );
    IF EXISTS (
        SELECT 1
        FROM public.compensation_pool_reversals AS reversal
        JOIN public.settlement_recoveries AS recovery
          ON recovery.id = reversal.recovery_id
        JOIN public.auctions AS auction ON auction.id = reversal.auction_id
        JOIN public.auction_settlements AS auction_settlement
          ON auction_settlement.auction_id = reversal.auction_id
        JOIN public.lots AS lot
          ON lot.id = reversal.lot_id AND lot.auction_id = reversal.auction_id
        JOIN public.lot_settlements AS lot_settlement
          ON lot_settlement.lot_id = reversal.lot_id
        WHERE reversal.recovery_id = checked_recovery_id
          AND (recovery.auction_id IS DISTINCT FROM reversal.auction_id
               OR recovery.lot_id IS DISTINCT FROM reversal.lot_id
               OR recovery.world_id IS DISTINCT FROM auction.world_id
               OR recovery.resolution IS DISTINCT FROM reversal.resolution
               OR lot_settlement.tax_amount IS DISTINCT FROM reversal.tax_amount
               OR lot_settlement.compensation_amount IS DISTINCT FROM reversal.compensation_amount)
    ) THEN
        RAISE EXCEPTION '补偿池冲正与 recovery/结算事实不一致'
            USING ERRCODE = '23514';
    END IF;
    RETURN NULL;
END
$function$;
CREATE CONSTRAINT TRIGGER compensation_reversal_matches_settlement_000120
AFTER INSERT ON public.compensation_pool_reversals
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.validate_compensation_reversal_000120();
CREATE CONSTRAINT TRIGGER recovery_keeps_compensation_reversal_identity_000120
AFTER UPDATE ON public.settlement_recoveries
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.validate_compensation_reversal_000120();
-- 000054 的独立 FK 未证明 seller/payment obligation 属于同一 lot 与合法 obligation kind。
DO $block$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM public.settlement_legacy_bot_seller_claims AS claim
        LEFT JOIN public.settlement_obligations AS seller
          ON seller.id = claim.obligation_id
        LEFT JOIN public.settlement_obligations AS payment
          ON payment.id = claim.payment_obligation_id
        WHERE seller.id IS NULL OR payment.id IS NULL
           OR seller.obligation_kind <> 'seller_payout'
           OR payment.obligation_kind NOT IN ('winner_equipment_cod', 'winner_payment_carrier_cod')
           OR seller.lot_id <> claim.lot_id OR payment.lot_id <> claim.lot_id
           OR seller.auction_id <> payment.auction_id
           OR seller.world_id <> payment.world_id
           OR NOT EXISTS (
               SELECT 1 FROM public.lots AS lot
               WHERE lot.id = claim.lot_id AND lot.auction_id = seller.auction_id
           )
           OR NOT (
               (payment.obligation_kind = 'winner_equipment_cod'
                   AND seller.prerequisite_obligation_id IS NOT DISTINCT FROM payment.id)
               OR (payment.obligation_kind = 'winner_payment_carrier_cod'
                   AND EXISTS (
                       SELECT 1 FROM public.settlement_obligations AS handoff
                       WHERE handoff.auction_id = payment.auction_id
                         AND handoff.lot_id = payment.lot_id
                         AND handoff.obligation_kind = 'winner_equipment_handoff'
                         AND seller.prerequisite_obligation_id = handoff.id
                   ))
           )
    ) THEN
        RAISE EXCEPTION '既有 legacy seller claim 的义务种类或 lot 关系无法证明'
            USING ERRCODE = '23514';
    END IF;
END
$block$;

CREATE FUNCTION public.validate_legacy_bot_seller_claim_000120()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
BEGIN
    -- 对依赖行加共享锁，避免校验后并发改写义务身份。
    PERFORM 1
    FROM public.settlement_obligations AS obligation
    WHERE obligation.id IN (NEW.obligation_id, NEW.payment_obligation_id)
    ORDER BY obligation.id
    FOR SHARE;
    IF NOT EXISTS (
        SELECT 1
        FROM public.settlement_obligations AS seller
        JOIN public.settlement_obligations AS payment
          ON payment.id = NEW.payment_obligation_id
        JOIN public.lots AS lot
          ON lot.id = NEW.lot_id AND lot.auction_id = seller.auction_id
        WHERE seller.id = NEW.obligation_id
          AND seller.obligation_kind = 'seller_payout'
          AND payment.obligation_kind IN ('winner_equipment_cod', 'winner_payment_carrier_cod')
          AND seller.lot_id = NEW.lot_id
          AND payment.lot_id = NEW.lot_id
          AND seller.auction_id = payment.auction_id
          AND seller.world_id = payment.world_id
          AND (
              (payment.obligation_kind = 'winner_equipment_cod'
                  AND seller.prerequisite_obligation_id = payment.id)
              OR (payment.obligation_kind = 'winner_payment_carrier_cod'
                  AND EXISTS (
                      SELECT 1 FROM public.settlement_obligations AS handoff
                      WHERE handoff.auction_id = payment.auction_id
                        AND handoff.lot_id = payment.lot_id
                        AND handoff.obligation_kind = 'winner_equipment_handoff'
                        AND seller.prerequisite_obligation_id = handoff.id
                  ))
          )
    ) THEN
        RAISE EXCEPTION 'legacy seller claim 必须绑定同一 lot 的 seller_payout 与 payment prerequisite'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$function$;
CREATE CONSTRAINT TRIGGER settlement_legacy_bot_seller_claim_identity_000120
AFTER INSERT ON public.settlement_legacy_bot_seller_claims
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.validate_legacy_bot_seller_claim_000120();

CREATE FUNCTION public.guard_claimed_settlement_obligation_identity_000120()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
BEGIN
    IF (NEW.auction_id, NEW.lot_id, NEW.world_id, NEW.obligation_kind,
        NEW.prerequisite_obligation_id)
         IS DISTINCT FROM (OLD.auction_id, OLD.lot_id, OLD.world_id, OLD.obligation_kind,
                           OLD.prerequisite_obligation_id)
       AND EXISTS (
           SELECT 1 FROM public.settlement_legacy_bot_seller_claims AS claim
           WHERE claim.obligation_id = OLD.id OR claim.payment_obligation_id = OLD.id
       ) THEN
        RAISE EXCEPTION 'legacy claim 已绑定的 settlement obligation 身份不可改写'
            USING ERRCODE = '55000';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER settlement_claimed_obligation_identity_000120
BEFORE UPDATE OF auction_id, lot_id, world_id, obligation_kind, prerequisite_obligation_id
ON public.settlement_obligations
FOR EACH ROW EXECUTE FUNCTION public.guard_claimed_settlement_obligation_identity_000120();

-- record_only carrier COD 的 1-credit 来源只绑定真实 Isekai 付款义务的 Main 派发。
-- 迁移前置校验依赖只追加派发历史；新 issue 另要求仍为当前派发且义务 ready。
CREATE FUNCTION public.settlement_record_carrier_source_valid_000120(
    checked_source_id uuid,
    require_current_state boolean
)
RETURNS boolean
LANGUAGE sql
STABLE
SET search_path = pg_catalog, public
AS $function$
    SELECT EXISTS (
        SELECT 1
        FROM public.settlement_funding_sources AS source
        JOIN public.settlement_funding_accounts AS account ON account.uid = source.uid
        JOIN public.deliveries AS delivery ON delivery.id = source.source_key
        JOIN public.settlement_obligations AS obligation
          ON delivery.origin_id = obligation.id::text
        JOIN public.settlement_obligation_fulfillments AS fulfillment
          ON fulfillment.obligation_id = obligation.id
        JOIN public.lots AS lot
          ON lot.id = obligation.lot_id
         AND lot.auction_id = obligation.auction_id
         AND lot.equipment_id = obligation.equipment_id
        JOIN public.lot_settlements AS settlement ON settlement.lot_id = lot.id
        JOIN public.equipment AS equipment ON equipment.id = obligation.equipment_id
        WHERE source.id = checked_source_id
          AND source.kind = 'record_carrier'
          AND source.amount = 1
          AND account.accounting_mode = 'record_only'
          AND (
              (require_current_state
               AND fulfillment.delivery_id = delivery.id
               AND obligation.business_state = 'ready')
              OR (
                  NOT require_current_state
                  AND EXISTS (
                      SELECT 1
                      FROM public.settlement_fulfillment_dispatches AS dispatch
                      WHERE dispatch.obligation_id = fulfillment.obligation_id
                        AND dispatch.delivery_id = delivery.id
                  )
              )
          )
          AND delivery.origin_kind = 'settlement'
          AND delivery.origin_id = obligation.id::text
          AND delivery.payload->>'operation' = 'send'
          AND delivery.payload->>'world' = 'main'
          AND delivery.payload->>'template' = 'settlement_payment_carrier'
          AND delivery.payload->>'credits' = '1'
          AND delivery.payload->>'cod' =
              (obligation.amount::numeric + obligation.carrier_credit::numeric)::text
          AND delivery.payload->'variables'->>'reference' = obligation.id::text
          AND delivery.payload->'variables'->>'amount' =
              (obligation.amount::numeric + obligation.carrier_credit::numeric)::text
          AND delivery.payload->'equipment_secret_refs' = '[]'::jsonb
          AND obligation.obligation_kind = 'winner_payment_carrier_cod'
          AND obligation.world_id = 'isekai'
          AND obligation.carrier_credit = 1
          AND equipment.world_id = 'isekai'
          AND fulfillment.dispatch_world = 'main'
          AND fulfillment.delivery_operation = 'send'
          AND settlement.buyer_amount = obligation.amount
    )
$function$;

CREATE FUNCTION public.settlement_record_opening_credit_valid_000120(
    opening_uid text, opening_credit_id uuid
)
RETURNS boolean
LANGUAGE sql
STABLE
SET search_path = pg_catalog, public
AS $function$
    SELECT EXISTS (
        SELECT 1
        FROM public.settlement_funding_allocations AS allocation
        JOIN public.settlement_funding_accounts AS account ON account.uid = allocation.uid
        WHERE allocation.id = opening_credit_id AND allocation.uid = opening_uid
          AND account.accounting_mode = 'record_only'
          AND allocation.purpose = 'quarantine'
          AND allocation.lot_id IS NULL AND allocation.obligation_id IS NULL
    )
$function$;

-- 000047 允许拆分合法来源；总发行不超额，returned_delivery 必须恰有一笔完整 return。
-- 旧版开账分录的摘要是命令摘要；按同一命令与 quarantine 用途核对历史发行。
DO $block$
BEGIN
    IF EXISTS (
        SELECT source.id
        FROM public.settlement_funding_sources AS source
        LEFT JOIN public.settlement_funding_entries AS entry ON entry.source_id = source.id
        GROUP BY source.id, source.amount
        HAVING COALESCE(sum(entry.amount::numeric)
                    FILTER (WHERE entry.kind IN ('issue', 'return')), 0)
                   > source.amount::numeric
            OR (
                COALESCE(bool_or(entry.kind = 'issue'), false)
                AND NOT (
                    source.kind IN ('reconciliation', 'collected_payment')
                    OR (
                        source.kind = 'record_opening'
                        AND COALESCE(bool_and(
                            entry.command_id = source.source_key
                            AND public.settlement_record_opening_credit_valid_000120(
                                source.uid, entry.credit_id
                            )
                        ) FILTER (WHERE entry.kind = 'issue' AND source.kind = 'record_opening'), false)
                    )
                    OR (
                        source.kind = 'record_carrier'
                        AND public.settlement_record_carrier_source_valid_000120(source.id, false)
                    )
                    OR (
                        source.kind IN (
                            'record_internal_purchase',
                            'record_operator_equipment_loss',
                            'record_compensation'
                        )
                        AND public.settlement_record_only_source_valid_000120(
                            source.id, NULL, false
                        )
                    )
                )
            )
    ) OR EXISTS (
        SELECT 1
        FROM public.settlement_funding_entries AS entry
        LEFT JOIN public.settlement_funding_sources AS source ON source.id = entry.source_id
        WHERE entry.kind = 'return'
          AND (source.id IS NULL OR source.kind <> 'returned_delivery'
               OR entry.amount <> source.amount)
    ) OR EXISTS (
        SELECT source.id
        FROM public.settlement_funding_sources AS source
        LEFT JOIN public.settlement_funding_entries AS entry ON entry.source_id = source.id
        WHERE source.kind = 'returned_delivery'
        GROUP BY source.id
        HAVING count(entry.id) <> 1
            OR count(entry.id) FILTER (WHERE entry.kind = 'return') <> 1
    ) THEN
        RAISE EXCEPTION '资金来源发行额度或 returned_delivery 的唯一 return 分录无法证明；须先人工核对'
            USING ERRCODE = '23514';
    END IF;
END
$block$;

-- issue 上限依赖历史分录不可修改；金额变化只能追加新分录。
CREATE FUNCTION public.reject_settlement_funding_entry_mutation_000120()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
BEGIN
    RAISE EXCEPTION '结算资金分录只允许追加写入'
        USING ERRCODE = '55000';
END
$function$;
CREATE TRIGGER settlement_funding_entries_append_only_000120
BEFORE UPDATE OR DELETE ON public.settlement_funding_entries
FOR EACH ROW EXECUTE FUNCTION public.reject_settlement_funding_entry_mutation_000120();
CREATE TRIGGER settlement_funding_entries_no_truncate_000120
BEFORE TRUNCATE ON public.settlement_funding_entries
FOR EACH STATEMENT EXECUTE FUNCTION public.reject_settlement_funding_entry_mutation_000120();

CREATE FUNCTION public.enforce_settlement_funding_issue_limit_000120()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
DECLARE
    source_uid text;
    source_kind text;
    source_key uuid;
    source_amount bigint;
    source_proof_digest bytea;
    already_issued numeric;
BEGIN
    IF NEW.kind NOT IN ('issue', 'return') THEN
        RETURN NEW;
    END IF;
    SELECT source.uid, source.kind, source.source_key, source.amount, source.proof_digest
    INTO source_uid, source_kind, source_key, source_amount, source_proof_digest
    FROM public.settlement_funding_sources AS source
    WHERE source.id = NEW.source_id
    FOR UPDATE;
    IF NOT FOUND OR source_uid IS DISTINCT FROM NEW.uid THEN
        RAISE EXCEPTION '资金分录来源不存在或 UID 不匹配'
            USING ERRCODE = '23514';
    END IF;
    IF NEW.kind = 'issue'
       AND source_kind NOT IN ('reconciliation', 'collected_payment')
       AND NOT (
           (source_kind = 'record_carrier'
            AND NEW.amount = 1
            AND public.settlement_record_carrier_source_valid_000120(NEW.source_id, true))
           OR (source_kind = 'record_opening'
               AND NEW.command_id = source_key
               AND NEW.proof_digest = source_proof_digest
               AND public.settlement_record_opening_credit_valid_000120(NEW.uid, NEW.credit_id))
           OR (
               source_kind IN (
                   'record_internal_purchase',
                   'record_operator_equipment_loss',
                   'record_compensation'
               )
               AND NEW.proof_digest = source_proof_digest
               AND public.settlement_record_only_source_valid_000120(
                   NEW.source_id, NEW.credit_id, true
               )
           )
       ) THEN
        RAISE EXCEPTION 'issue 来源必须匹配冻结结算与 record_only 责任事实'
            USING ERRCODE = '23514';
    END IF;
    IF NEW.kind = 'return' THEN
        IF source_kind <> 'returned_delivery' OR NEW.amount <> source_amount
           OR EXISTS (
               SELECT 1 FROM public.settlement_funding_entries AS entry
               WHERE entry.source_id = NEW.source_id
           ) THEN
            RAISE EXCEPTION 'returned_delivery 来源必须唯一发行一笔完整 return'
                USING ERRCODE = '23514';
        END IF;
    END IF;
    SELECT COALESCE(sum(entry.amount::numeric), 0)
    INTO already_issued
    FROM public.settlement_funding_entries AS entry
    WHERE entry.source_id = NEW.source_id
      AND entry.kind IN ('issue', 'return');
    IF already_issued + NEW.amount::numeric > source_amount::numeric THEN
        RAISE EXCEPTION '资金来源分录超过冻结额度'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER settlement_funding_issue_limit_000120
BEFORE INSERT ON public.settlement_funding_entries
FOR EACH ROW EXECUTE FUNCTION public.enforce_settlement_funding_issue_limit_000120();
INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (120, 'breaking');
