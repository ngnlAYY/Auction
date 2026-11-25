-- 寄售第 6 期：购买、资金、退件接管与拒收。
-- owner: auction-storage（购买状态机、资金收款与费用、派发轮次、退件接管与消费、拒收与处罚、维护授权）
-- 不变量 1、2、3、4、5、6、8、10：命令账本与回执形状闭集；购买事实（合同、义务图、挂单与 position 迁移）同事务绑定；
-- 买家确领原件才释放实物占用（新证明表只追加）；金额守恒 G = A + U、0 <= A <= G，每段 opening 份额只消费一次；
-- 现金退件未核清前该 UID 不得建立 opening；同一退件、同一起写、同一轮只发生一次；获证判阈值的处罚与新购阻断同事务。
-- 等级：breaking。放宽挂单、position、代管事件与交接边的守卫，改写资金分配唯一键并新增来源种类，
-- 旧程序读不懂新态与新键，须在停机门禁下与同版程序一起切换。
--
-- 守卫函数整函数替换：c4_guard_position、c4_guard_claim_fact 以 000135 的实际文本为底，
-- consignment_guard_listing、consignment_check_listing_event、c4_valid_v2_receipt 以 000134 的文本为底
-- （135 未改写它们），与底本的差异只有各节注明的新增分支。consignment_check_listing_event 的 v2 回执 closed 分支
-- 为新增 prove_handoff 而把括号移到外层；withdraw / admin_delist 仍与 000134 一样核对 result_body 的
-- listing_id、listing_version、state 与 disposition = 'closed'，语义等价。
-- 开工前核实：
--   1. 资金 allocations 的 ON CONFLICT (uid,purpose,lot_id) 目标随唯一键改为四列，存储层 ledger.rs 同步。
--   2. 单实物同一挂单只有一份合同（c5_contract_generation_uq），购买“消耗销售代”由合同行表达，不新增代表列。
--   3. v2 回执的目标种类沿 request_target = target_id::text；周期与实收业务键使用确定性 UUIDv5。
-- 6B3 返工登记（未发布迁移原地修改）：record_only 寄售载体本金首封的 record_carrier 来源可验。
-- 000120 的 settlement_record_carrier_source_valid_000120 只认拍卖 lot 与载体义务，寄售义务没有 lot，判定恒为假；
-- 本文件新增 settlement_record_carrier_consignment_valid_000136，并只在 enforce_settlement_funding_issue_limit_000120
-- 的 record_carrier 分支追加 OR 它；拍卖分支（对 000120 函数的调用）与其余分支逐字不变。
-- 6B2 返工登记（未发布迁移原地修改）：c6_guard_opening_slice() 改为 SECURITY DEFINER 并固定 search_path，
-- 因运行账号登记 opening 份额时，守卫要读写运行账号无权直接改写的行；与 000131 的同类守卫先例一致。

LOCK TABLE
    public.settlement_obligations,
    public.settlement_funding_allocations,
    public.settlement_funding_entries,
    public.settlement_funding_sources,
    public.settlement_funding_receipts,
    public.consignment_contracts,
    public.consignment_sales,
    public.consignment_commands,
    public.consignment_listings,
    public.consignment_events,
    public.consignment_custody_positions,
    public.consignment_custody_events,
    public.consignment_equipment_handoffs,
    public.delivery_sent_bindings,
    public.mail_disposition_proofs,
    public.mail_final_dispositions,
    public.mail_initial_decisions,
    public.mail_quarantine_releases
IN SHARE ROW EXCLUSIVE MODE;

-- ① 既有表上的支撑唯一约束：只为新表的复合外键服务，不改变旧列、旧约束或旧写入语义。
ALTER TABLE public.settlement_obligations
    ADD CONSTRAINT c6_obligation_contract_identity_uq UNIQUE (id, contract_id);
ALTER TABLE public.consignment_contracts
    ADD CONSTRAINT c6_contract_position_generation_uq UNIQUE (contract_id, position_id, generation);
ALTER TABLE public.delivery_sent_bindings
    ADD CONSTRAINT c6_sent_binding_identity_uq
        UNIQUE (world_id, external_mail_id, delivery_id, fencing_version);
ALTER TABLE public.settlement_funding_receipts
    ADD CONSTRAINT c6_receipt_identity_uq
        UNIQUE (source_id, uid, payment_obligation_id, delivery_id);
ALTER TABLE public.mail_disposition_proofs
    ADD CONSTRAINT c6_proof_take_identity_uq UNIQUE (id, work_id, delivery_id, fence);


-- ② 买家确领原件的证明：只追加，对 (合同, 义务) 唯一；占用释放与 handed_over 迁移只认本表。
CREATE TABLE public.consignment_handoff_proofs (
    id uuid NOT NULL CONSTRAINT c6_handoff_proof_pk PRIMARY KEY,
    position_id uuid NOT NULL,
    generation bigint NOT NULL,
    contract_id uuid NOT NULL,
    obligation_id uuid NOT NULL,
    delivery_id uuid NOT NULL,
    fence bigint NOT NULL,
    validator_version text NOT NULL,
    proof_digest bytea NOT NULL,
    observed_at timestamptz NOT NULL,
    recorded_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CONSTRAINT c6_handoff_proof_obligation_uq UNIQUE (contract_id, obligation_id),
    CONSTRAINT c6_handoff_proof_contract_fk FOREIGN KEY (contract_id, position_id, generation)
        REFERENCES public.consignment_contracts (contract_id, position_id, generation)
        ON DELETE RESTRICT,
    CONSTRAINT c6_handoff_proof_obligation_fk FOREIGN KEY (obligation_id, contract_id)
        REFERENCES public.settlement_obligations (id, contract_id) ON DELETE RESTRICT,
    CONSTRAINT c6_handoff_proof_attempt_fk FOREIGN KEY (delivery_id, fence)
        REFERENCES public.delivery_send_attempts (delivery_id, fencing_version)
        ON DELETE RESTRICT,
    CONSTRAINT c6_handoff_proof_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c6_handoff_proof_fence_ck CHECK (fence BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c6_handoff_proof_validator_ck CHECK (public.consignment_valid_text(validator_version, 64)),
    CONSTRAINT c6_handoff_proof_digest_ck CHECK (public.c4_valid_digest(proof_digest)),
    CONSTRAINT c6_handoff_proof_time_ck CHECK (isfinite(observed_at))
);
CREATE INDEX c6_handoff_proof_position_idx
    ON public.consignment_handoff_proofs (position_id, generation);

CREATE FUNCTION public.c6_guard_handoff_proof() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    -- 原件确领只对 main 的 WinnerEquipmentCod 与 isekai 的 WinnerEquipmentHandoff 成立。
    IF NOT EXISTS (
        SELECT 1 FROM public.settlement_obligations AS obligation
        WHERE obligation.id = NEW.obligation_id
          AND obligation.contract_id = NEW.contract_id
          AND obligation.obligation_kind IN ('winner_equipment_cod', 'winner_equipment_handoff')
    ) THEN
        RAISE EXCEPTION '确领证明只能绑定原件义务'
            USING ERRCODE = '23514', CONSTRAINT = 'c6_handoff_proof_obligation_kind';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c6_handoff_proof_binding
BEFORE INSERT ON public.consignment_handoff_proofs
FOR EACH ROW EXECUTE FUNCTION public.c6_guard_handoff_proof();
CREATE TRIGGER c6_handoff_proofs_append_only
BEFORE UPDATE OR DELETE ON public.consignment_handoff_proofs
FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER c6_handoff_proofs_no_truncate
BEFORE TRUNCATE ON public.consignment_handoff_proofs
FOR EACH STATEMENT EXECUTE FUNCTION public.consignment_reject_fact_mutation();

-- 交接边放宽 consignment_delivered（consignment_withdrawn / consignment_recovered 留第 7 期）。
ALTER TABLE public.consignment_equipment_handoffs
    DROP CONSTRAINT c4_handoff_edge_phase4_ck,
    ADD CONSTRAINT c6_handoff_edge_kinds_ck CHECK (kind IN (
        'custody_acquired', 'auction_winner_collected', 'auction_unsold_returned',
        'auction_withdrawn_returned', 'consignment_delivered'
    )),
    ADD CONSTRAINT c6_handoff_edge_position_kind_ck CHECK (
        kind <> 'consignment_delivered' OR position_id IS NOT NULL
    );

-- 代管事件：新增已承诺 / 在途 / 已交付三种事件，handed_over 事件绑定确领证明（不是邮件证明）；
-- 季末终态额外允许 awaiting_take（未起写的取得工作在季末关闭）。lost 仍由第 7 期放开。
ALTER TABLE public.consignment_custody_events
    ADD COLUMN handoff_proof_id uuid CONSTRAINT c6_custody_event_handoff_proof_fk
        REFERENCES public.consignment_handoff_proofs(id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED;
ALTER TABLE public.consignment_custody_events
    DROP CONSTRAINT c5_custody_event_kinds_ck,
    DROP CONSTRAINT c5_custody_event_season_ended_ck,
    DROP CONSTRAINT c4_custody_event_state_ck,
    ADD CONSTRAINT c6_custody_event_kinds_ck CHECK (
        event_kind IN ('accepted', 'in_custody', 'reconcile', 'committed', 'in_transit',
                       'handed_over', 'season_ended')
    ),
    ADD CONSTRAINT c4_custody_event_state_ck CHECK ((
        CASE event_kind
            WHEN 'accepted' THEN from_state IS NULL AND to_state = 'awaiting_take'
            WHEN 'in_custody' THEN from_state IN ('awaiting_take', 'reconcile')
                AND to_state = 'in_custody'
            WHEN 'reconcile' THEN from_state IN ('in_custody', 'committed', 'in_transit')
                AND to_state = 'reconcile'
            WHEN 'committed' THEN from_state IN ('in_custody', 'in_transit', 'reconcile')
                AND to_state = 'committed'
            WHEN 'in_transit' THEN from_state IN ('committed', 'reconcile')
                AND to_state = 'in_transit'
            WHEN 'handed_over' THEN from_state IN ('in_transit', 'committed')
                AND to_state = 'handed_over'
            WHEN 'season_ended' THEN from_state IS NOT NULL AND to_state = 'season_ended'
            WHEN 'lost' THEN from_state IS NOT NULL AND to_state = 'lost'
        END
    ) IS TRUE),
    ADD CONSTRAINT c6_custody_event_season_ended_ck CHECK (
        event_kind <> 'season_ended'
        OR (from_state IN ('awaiting_take', 'in_custody', 'reconcile') AND proof_id IS NULL)
    ),
    ADD CONSTRAINT c6_custody_event_handoff_ck CHECK (
        (event_kind = 'handed_over') = (handoff_proof_id IS NOT NULL)
        AND (event_kind <> 'handed_over' OR proof_id IS NULL)
    );


-- ③ 派发轮次、尝试与证据。尝试复用 delivery_send_attempts / delivery_sent_bindings（冻结 Send 身份与跨来源
-- 唯一 Sent MID 绑定），本节只加寄售的轮、冻结身份与追加证据。
CREATE TABLE public.consignment_dispatch_rounds (
    contract_id uuid NOT NULL,
    obligation_id uuid NOT NULL,
    round integer NOT NULL,
    round_id uuid NOT NULL,
    delivery_id uuid NOT NULL CONSTRAINT c6_round_delivery_fk
        REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    rights_holder_identity text NOT NULL,
    bot_uid text NOT NULL,
    world_id text NOT NULL,
    subject text NOT NULL,
    template text NOT NULL,
    body_digest bytea NOT NULL,
    round_ref text NOT NULL,
    d0 timestamptz NOT NULL,
    credits bigint NOT NULL,
    cod bigint NOT NULL,
    prior_round integer,
    authorized_by_intake uuid,
    authorized_by_consumption uuid,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CONSTRAINT c6_round_pk PRIMARY KEY (contract_id, obligation_id, round),
    CONSTRAINT c6_round_id_uq UNIQUE (round_id),
    CONSTRAINT c6_round_delivery_uq UNIQUE (delivery_id),
    CONSTRAINT c6_round_identity_uq UNIQUE (contract_id, obligation_id, round, delivery_id),
    CONSTRAINT c6_round_obligation_fk FOREIGN KEY (obligation_id, contract_id)
        REFERENCES public.settlement_obligations (id, contract_id) ON DELETE RESTRICT,
    CONSTRAINT c6_round_id_ck CHECK (public.c4_valid_uuid(round_id)),
    CONSTRAINT c6_round_number_ck CHECK (round BETWEEN 1 AND 100),
    -- 首轮没有授权来源；新轮必须引用封闭上轮的退件接管与消费（同一合同、义务，轮号相邻）。
    CONSTRAINT c6_round_authorization_ck CHECK (
        (round = 1 AND prior_round IS NULL AND authorized_by_intake IS NULL
            AND authorized_by_consumption IS NULL)
        OR (round > 1 AND prior_round = round - 1 AND authorized_by_intake IS NOT NULL
            AND authorized_by_consumption IS NOT NULL)
    ),
    CONSTRAINT c6_round_identity_text_ck CHECK (
        public.consignment_valid_text(rights_holder_identity, 128)
        AND public.consignment_valid_text(subject, 512)
        AND public.consignment_valid_text(template, 64)
        AND public.consignment_valid_text(round_ref, 64)
        AND round_ref = round_id::text
    ),
    CONSTRAINT c6_round_world_ck CHECK (world_id IN ('main', 'isekai')),
    CONSTRAINT c6_round_bot_uid_ck CHECK (bot_uid COLLATE "C" ~ '^[1-9][0-9]{0,31}$'),
    CONSTRAINT c6_round_digest_ck CHECK (public.c4_valid_digest(body_digest)),
    CONSTRAINT c6_round_amount_ck CHECK (credits >= 0 AND cod >= 0),
    CONSTRAINT c6_round_time_ck CHECK (isfinite(d0) AND isfinite(created_at))
);

CREATE TABLE public.consignment_dispatch_attempts (
    delivery_id uuid NOT NULL,
    fence bigint NOT NULL,
    attempt_id uuid NOT NULL,
    contract_id uuid NOT NULL,
    obligation_id uuid NOT NULL,
    round integer NOT NULL,
    leg text NOT NULL,
    started_at timestamptz NOT NULL,
    CONSTRAINT c6_attempt_pk PRIMARY KEY (delivery_id, fence),
    CONSTRAINT c6_attempt_id_uq UNIQUE (attempt_id),
    CONSTRAINT c6_attempt_identity_uq UNIQUE (attempt_id, delivery_id, fence),
    CONSTRAINT c6_attempt_round_fk FOREIGN KEY (contract_id, obligation_id, round, delivery_id)
        REFERENCES public.consignment_dispatch_rounds (contract_id, obligation_id, round, delivery_id)
        ON DELETE RESTRICT,
    CONSTRAINT c6_attempt_send_fk FOREIGN KEY (delivery_id, fence)
        REFERENCES public.delivery_send_attempts (delivery_id, fencing_version)
        ON DELETE RESTRICT,
    CONSTRAINT c6_attempt_id_ck CHECK (public.c4_valid_uuid(attempt_id)),
    CONSTRAINT c6_attempt_fence_ck CHECK (fence BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c6_attempt_leg_ck CHECK (leg IN (
        'original', 'carrier', 'cash_payout', 'cash_refund')),
    CONSTRAINT c6_attempt_time_ck CHECK (isfinite(started_at))
);

CREATE TABLE public.consignment_dispatch_evidence (
    id uuid NOT NULL CONSTRAINT c6_evidence_pk PRIMARY KEY,
    attempt_id uuid NOT NULL CONSTRAINT c6_evidence_attempt_fk
        REFERENCES public.consignment_dispatch_attempts (attempt_id) ON DELETE RESTRICT,
    category text NOT NULL,
    remote_observation_id text NOT NULL,
    validator_version text NOT NULL,
    digest bytea NOT NULL,
    observed_at timestamptz NOT NULL,
    CONSTRAINT c6_evidence_observation_uq UNIQUE (attempt_id, category, remote_observation_id),
    CONSTRAINT c6_evidence_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c6_evidence_category_ck CHECK (category IN (
        'sent_binding', 'original_claim', 'carrier_payment', 'cash_claim', 'rts')),
    CONSTRAINT c6_evidence_observation_ck CHECK (
        public.consignment_safe_observation_text(remote_observation_id, 128)),
    CONSTRAINT c6_evidence_validator_ck CHECK (public.consignment_valid_text(validator_version, 64)),
    CONSTRAINT c6_evidence_digest_ck CHECK (public.c4_valid_digest(digest)),
    CONSTRAINT c6_evidence_time_ck CHECK (isfinite(observed_at))
);

-- ④ 退件接管：按 RTS 的原信键 K 恰一个 intake，同一合同、义务、原轮、同一应退腿只接一件。
CREATE TABLE public.consignment_return_intakes (
    id uuid NOT NULL CONSTRAINT c6_intake_pk PRIMARY KEY,
    bot_uid text NOT NULL,
    world_id text NOT NULL,
    external_mid text NOT NULL,
    decision_id uuid NOT NULL,
    decision_kind text NOT NULL DEFAULT 'acquire_return',
    contract_id uuid NOT NULL,
    obligation_id uuid NOT NULL,
    original_round integer NOT NULL,
    original_delivery_id uuid NOT NULL,
    original_fence bigint NOT NULL,
    sent_mid text NOT NULL,
    leg text NOT NULL,
    rts_observation_id uuid NOT NULL CONSTRAINT c6_intake_observation_fk
        REFERENCES public.mail_envelope_observations(id) ON DELETE RESTRICT,
    rts_returned_at timestamptz NOT NULL,
    manifest_digest bytea NOT NULL,
    session_id uuid NOT NULL CONSTRAINT c6_intake_session_fk
        REFERENCES public.mail_evidence_sessions(id) ON DELETE RESTRICT,
    take_work_id uuid NOT NULL CONSTRAINT c6_intake_work_fk
        REFERENCES public.mail_disposition_work(id) ON DELETE RESTRICT,
    take_delivery_id uuid NOT NULL CONSTRAINT c6_intake_take_delivery_fk
        REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CONSTRAINT c6_intake_key_uq UNIQUE (bot_uid, world_id, external_mid),
    CONSTRAINT c6_intake_decision_uq UNIQUE (decision_id),
    CONSTRAINT c6_intake_work_uq UNIQUE (take_work_id),
    CONSTRAINT c6_intake_take_delivery_uq UNIQUE (take_delivery_id),
    CONSTRAINT c6_intake_leg_uq UNIQUE (contract_id, obligation_id, original_round, leg),
    CONSTRAINT c6_intake_identity_uq UNIQUE (id, contract_id, obligation_id, original_round, leg),
    CONSTRAINT c6_intake_round_uq UNIQUE (id, contract_id, obligation_id, original_round),
    CONSTRAINT c6_intake_decision_fk FOREIGN KEY (bot_uid, world_id, external_mid, decision_id, decision_kind)
        REFERENCES public.mail_final_dispositions (bot_uid, world_id, external_mid, id, kind)
        ON DELETE RESTRICT,
    CONSTRAINT c6_intake_round_fk FOREIGN KEY (contract_id, obligation_id, original_round, original_delivery_id)
        REFERENCES public.consignment_dispatch_rounds (contract_id, obligation_id, round, delivery_id)
        ON DELETE RESTRICT,
    CONSTRAINT c6_intake_sent_fk FOREIGN KEY (world_id, sent_mid, original_delivery_id, original_fence)
        REFERENCES public.delivery_sent_bindings (world_id, external_mail_id, delivery_id, fencing_version)
        ON DELETE RESTRICT,
    CONSTRAINT c6_intake_kind_ck CHECK (decision_kind = 'acquire_return'),
    CONSTRAINT c6_intake_key_ck CHECK (
        bot_uid COLLATE "C" ~ '^[1-9][0-9]{0,31}$'
        AND external_mid COLLATE "C" ~ '^[1-9][0-9]{0,31}$'
        AND sent_mid COLLATE "C" ~ '^[1-9][0-9]{0,31}$'
        AND world_id IN ('main', 'isekai')
    ),
    CONSTRAINT c6_intake_mail_distinct_ck CHECK (external_mid <> sent_mid),
    CONSTRAINT c6_intake_leg_ck CHECK (leg IN (
        'original', 'carrier', 'cash_refund', 'cash_payout', 'cash_principal')),
    CONSTRAINT c6_intake_round_ck CHECK (original_round BETWEEN 1 AND 100
        AND original_fence BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c6_intake_digest_ck CHECK (public.c4_valid_digest(manifest_digest)),
    CONSTRAINT c6_intake_time_ck CHECK (isfinite(rts_returned_at) AND isfinite(created_at))
);

-- 第二个不同 MID 主张同一轮同一腿：进冲突案，不接管。
CREATE TABLE public.consignment_return_conflicts (
    bot_uid text NOT NULL,
    world_id text NOT NULL,
    external_mid text NOT NULL,
    intake_id uuid NOT NULL CONSTRAINT c6_conflict_intake_fk
        REFERENCES public.consignment_return_intakes(id) ON DELETE RESTRICT,
    observation_id uuid NOT NULL CONSTRAINT c6_conflict_observation_fk
        REFERENCES public.mail_envelope_observations(id) ON DELETE RESTRICT,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CONSTRAINT c6_conflict_pk PRIMARY KEY (bot_uid, world_id, external_mid),
    CONSTRAINT c6_conflict_key_ck CHECK (
        bot_uid COLLATE "C" ~ '^[1-9][0-9]{0,31}$'
        AND external_mid COLLATE "C" ~ '^[1-9][0-9]{0,31}$'
        AND world_id IN ('main', 'isekai')
    ),
    CONSTRAINT c6_conflict_time_ck CHECK (isfinite(created_at))
);

-- ⑤ 退件消费：唯一 K、唯一 intake、唯一原轮资产腿，复合绑定该 intake 的起写与 Take 证明。
CREATE TABLE public.consignment_return_consumptions (
    id uuid NOT NULL CONSTRAINT c6_consumption_pk PRIMARY KEY,
    intake_id uuid NOT NULL,
    contract_id uuid NOT NULL,
    obligation_id uuid NOT NULL,
    original_round integer NOT NULL,
    leg text NOT NULL,
    bot_uid text NOT NULL,
    world_id text NOT NULL,
    external_mid text NOT NULL,
    work_id uuid NOT NULL,
    delivery_id uuid NOT NULL,
    attempt_id uuid NOT NULL,
    fence bigint NOT NULL,
    proof_id uuid NOT NULL,
    consumed_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CONSTRAINT c6_consumption_intake_uq UNIQUE (intake_id),
    CONSTRAINT c6_consumption_key_uq UNIQUE (bot_uid, world_id, external_mid),
    CONSTRAINT c6_consumption_leg_uq UNIQUE (contract_id, obligation_id, original_round, leg),
    CONSTRAINT c6_consumption_proof_uq UNIQUE (proof_id),
    CONSTRAINT c6_consumption_identity_uq UNIQUE (id, intake_id),
    CONSTRAINT c6_consumption_full_identity_uq UNIQUE (id, intake_id, contract_id, leg),
    CONSTRAINT c6_consumption_intake_fk
        FOREIGN KEY (intake_id, contract_id, obligation_id, original_round, leg)
        REFERENCES public.consignment_return_intakes (id, contract_id, obligation_id, original_round, leg)
        ON DELETE RESTRICT,
    CONSTRAINT c6_consumption_key_fk FOREIGN KEY (bot_uid, world_id, external_mid)
        REFERENCES public.consignment_return_intakes (bot_uid, world_id, external_mid)
        ON DELETE RESTRICT,
    CONSTRAINT c6_consumption_start_fk FOREIGN KEY (work_id, delivery_id, attempt_id, fence)
        REFERENCES public.mail_disposition_write_starts (work_id, delivery_id, attempt_id, fence)
        ON DELETE RESTRICT,
    CONSTRAINT c6_consumption_proof_fk FOREIGN KEY (proof_id, work_id, delivery_id, fence)
        REFERENCES public.mail_disposition_proofs (id, work_id, delivery_id, fence)
        ON DELETE RESTRICT,
    CONSTRAINT c6_consumption_time_ck CHECK (isfinite(consumed_at))
);

-- 新轮的授权来源在两表建好后补外键：round N+1 引用 round N 的 intake 与 consumption。
ALTER TABLE public.consignment_dispatch_rounds
    ADD CONSTRAINT c6_round_intake_fk
        FOREIGN KEY (authorized_by_intake, contract_id, obligation_id, prior_round)
        REFERENCES public.consignment_return_intakes (id, contract_id, obligation_id, original_round)
        ON DELETE RESTRICT,
    ADD CONSTRAINT c6_round_consumption_fk
        FOREIGN KEY (authorized_by_consumption, authorized_by_intake)
        REFERENCES public.consignment_return_consumptions (id, intake_id)
        ON DELETE RESTRICT;

CREATE FUNCTION public.c6_guard_return_consumption() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    -- 消费必须在同一事务里引用该 intake 唯一 Take 工作上的 take_proven 证明与起写，
    -- 证明属于同一 K 的决定；只拿到邮件或处罚事实不足以消费。
    IF NOT EXISTS (
        SELECT 1
        FROM public.consignment_return_intakes AS intake
        JOIN public.mail_disposition_proofs AS proof ON proof.id = NEW.proof_id
        WHERE intake.id = NEW.intake_id
          AND intake.take_work_id = NEW.work_id
          AND intake.take_delivery_id = NEW.delivery_id
          AND intake.decision_id = proof.decision_id
          AND proof.work_id = NEW.work_id
          AND proof.kind = 'exact_take'
          AND proof.conclusion = 'take_proven'
          AND proof.xmin = pg_current_xact_id()::xid
    ) THEN
        RAISE EXCEPTION '退件消费缺少本事务写入的精确 Take 证明'
            USING ERRCODE = '23514', CONSTRAINT = 'c6_consumption_take_proof';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c6_consumption_binding
BEFORE INSERT ON public.consignment_return_consumptions
FOR EACH ROW EXECUTE FUNCTION public.c6_guard_return_consumption();

-- ⑥ 拒收周期与拒收事实（返回事实不可变，裁决只追加）。
CREATE TABLE public.consignment_refusal_cycles (
    id uuid NOT NULL CONSTRAINT c6_cycle_pk PRIMARY KEY,
    buyer_user_id uuid NOT NULL CONSTRAINT c6_cycle_buyer_fk
        REFERENCES public.users(id) ON DELETE RESTRICT,
    ordinal bigint NOT NULL,
    state text NOT NULL DEFAULT 'open',
    opened_at timestamptz NOT NULL,
    closed_at timestamptz,
    close_reason text,
    CONSTRAINT c6_cycle_buyer_ordinal_uq UNIQUE (buyer_user_id, ordinal),
    CONSTRAINT c6_cycle_buyer_identity_uq UNIQUE (id, buyer_user_id),
    CONSTRAINT c6_cycle_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c6_cycle_ordinal_ck CHECK (ordinal BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c6_cycle_state_ck CHECK (
        (state = 'open' AND closed_at IS NULL AND close_reason IS NULL)
        OR (state = 'closed' AND closed_at IS NOT NULL
            AND public.consignment_valid_text(close_reason, 64))
    ),
    CONSTRAINT c6_cycle_time_ck CHECK (
        isfinite(opened_at) AND (closed_at IS NULL OR (isfinite(closed_at) AND closed_at >= opened_at))
    )
);
-- 买家同一时刻至多一个 open 周期。
CREATE UNIQUE INDEX c6_cycle_open_uq
    ON public.consignment_refusal_cycles (buyer_user_id) WHERE state = 'open';

CREATE TABLE public.consignment_return_facts (
    return_fact_id uuid NOT NULL CONSTRAINT c6_fact_pk PRIMARY KEY,
    intake_id uuid NOT NULL CONSTRAINT c6_fact_intake_fk
        REFERENCES public.consignment_return_intakes(id) ON DELETE RESTRICT,
    contract_id uuid NOT NULL,
    obligation_id uuid NOT NULL,
    original_round integer NOT NULL,
    leg text NOT NULL,
    kind text NOT NULL,
    buyer_user_id uuid NOT NULL CONSTRAINT c6_fact_buyer_fk
        REFERENCES public.users(id) ON DELETE RESTRICT,
    cycle_id uuid NOT NULL,
    returned_at timestamptz NOT NULL,
    returned_minute_end timestamptz NOT NULL,
    proven_at timestamptz NOT NULL,
    delivery_close_at timestamptz,
    CONSTRAINT c6_fact_intake_uq UNIQUE (intake_id),
    CONSTRAINT c6_fact_leg_uq UNIQUE (contract_id, obligation_id, original_round, leg),
    CONSTRAINT c6_fact_member_uq UNIQUE (return_fact_id, contract_id),
    CONSTRAINT c6_fact_cycle_fk FOREIGN KEY (cycle_id, buyer_user_id)
        REFERENCES public.consignment_refusal_cycles (id, buyer_user_id) ON DELETE RESTRICT,
    CONSTRAINT c6_fact_intake_identity_fk
        FOREIGN KEY (intake_id, contract_id, obligation_id, original_round, leg)
        REFERENCES public.consignment_return_intakes (id, contract_id, obligation_id, original_round, leg)
        ON DELETE RESTRICT,
    CONSTRAINT c6_fact_id_ck CHECK (public.c4_valid_uuid(return_fact_id)),
    CONSTRAINT c6_fact_kind_ck CHECK (kind IN ('main_cod_return', 'isekai_unpaid_carrier_return')),
    CONSTRAINT c6_fact_close_shape_ck CHECK (
        (kind = 'main_cod_return' AND delivery_close_at IS NULL)
        OR (kind = 'isekai_unpaid_carrier_return' AND delivery_close_at IS NOT NULL)
    ),
    CONSTRAINT c6_fact_time_ck CHECK (
        isfinite(returned_at) AND isfinite(proven_at)
        AND returned_minute_end = returned_at + interval '1 minute'
        AND (delivery_close_at IS NULL OR isfinite(delivery_close_at))
    )
);

CREATE TABLE public.consignment_return_fact_verdicts (
    id uuid NOT NULL CONSTRAINT c6_verdict_pk PRIMARY KEY,
    return_fact_id uuid NOT NULL CONSTRAINT c6_verdict_fact_fk
        REFERENCES public.consignment_return_facts(return_fact_id) ON DELETE RESTRICT,
    verdict text NOT NULL,
    reason text,
    evaluated_at timestamptz NOT NULL,
    CONSTRAINT c6_verdict_identity_uq UNIQUE (id, return_fact_id),
    CONSTRAINT c6_verdict_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c6_verdict_shape_ck CHECK (
        (verdict = 'qualified' AND reason IS NULL)
        OR (verdict = 'not_counted' AND reason IN (
            'after_delivery_closed', 'paid_or_opposite', 'wrong_asset', 'duplicate'))
        OR (verdict = 'pending' AND reason IN (
            'boundary_minute_ambiguous', 'deliverability_disputed', 'ownership_unproven'))
    ),
    CONSTRAINT c6_verdict_time_ck CHECK (isfinite(evaluated_at))
);
-- 每个返回事实至多一次“合格”裁决；待判与不计可以随新观察追加。
CREATE UNIQUE INDEX c6_verdict_qualified_uq
    ON public.consignment_return_fact_verdicts (return_fact_id) WHERE verdict = 'qualified';

CREATE TABLE public.consignment_refusals (
    id uuid NOT NULL CONSTRAINT c6_refusal_pk PRIMARY KEY,
    return_fact_id uuid NOT NULL,
    verdict_id uuid NOT NULL,
    buyer_user_id uuid NOT NULL CONSTRAINT c6_refusal_buyer_fk
        REFERENCES public.users(id) ON DELETE RESTRICT,
    cycle_id uuid NOT NULL,
    contract_id uuid NOT NULL,
    kind text NOT NULL,
    qualified_at timestamptz NOT NULL,
    CONSTRAINT c6_refusal_fact_uq UNIQUE (return_fact_id),
    CONSTRAINT c6_refusal_fact_fk FOREIGN KEY (return_fact_id, contract_id)
        REFERENCES public.consignment_return_facts (return_fact_id, contract_id) ON DELETE RESTRICT,
    CONSTRAINT c6_refusal_verdict_fk FOREIGN KEY (verdict_id, return_fact_id)
        REFERENCES public.consignment_return_fact_verdicts (id, return_fact_id) ON DELETE RESTRICT,
    CONSTRAINT c6_refusal_cycle_fk FOREIGN KEY (cycle_id, buyer_user_id)
        REFERENCES public.consignment_refusal_cycles (id, buyer_user_id) ON DELETE RESTRICT,
    CONSTRAINT c6_refusal_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c6_refusal_kind_ck CHECK (kind IN ('main_cod_return', 'isekai_unpaid_carrier_return')),
    CONSTRAINT c6_refusal_time_ck CHECK (isfinite(qualified_at))
);

CREATE FUNCTION public.c6_guard_refusal() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    -- 拒收只能引用本事务可见、判为 qualified 的裁决，且买家、周期、种类与返回事实一致。
    IF NOT EXISTS (
        SELECT 1
        FROM public.consignment_return_fact_verdicts AS verdict
        JOIN public.consignment_return_facts AS fact ON fact.return_fact_id = verdict.return_fact_id
        WHERE verdict.id = NEW.verdict_id
          AND verdict.verdict = 'qualified'
          AND fact.buyer_user_id = NEW.buyer_user_id
          AND fact.cycle_id = NEW.cycle_id
          AND fact.kind = NEW.kind
    ) THEN
        RAISE EXCEPTION '拒收必须绑定合格裁决且与返回事实一致'
            USING ERRCODE = '23514', CONSTRAINT = 'c6_refusal_qualified';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c6_refusal_binding
BEFORE INSERT ON public.consignment_refusals
FOR EACH ROW EXECUTE FUNCTION public.c6_guard_refusal();

-- ⑦ 处罚、构成项与唯一待执行账号工作。pending 的处罚就是新购阻断。
CREATE TABLE public.consignment_sanctions (
    id uuid NOT NULL CONSTRAINT c6_sanction_pk PRIMARY KEY,
    buyer_user_id uuid NOT NULL CONSTRAINT c6_sanction_buyer_fk
        REFERENCES public.users(id) ON DELETE RESTRICT,
    cycle_id uuid NOT NULL,
    rule text NOT NULL,
    t0 timestamptz NOT NULL,
    threshold smallint NOT NULL,
    window_from timestamptz,
    window_to timestamptz,
    trigger_return_fact_id uuid NOT NULL CONSTRAINT c6_sanction_trigger_fk
        REFERENCES public.consignment_return_facts(return_fact_id) ON DELETE RESTRICT,
    state text NOT NULL DEFAULT 'pending',
    version bigint NOT NULL DEFAULT 1,
    CONSTRAINT c6_sanction_cycle_uq UNIQUE (buyer_user_id, cycle_id),
    CONSTRAINT c6_sanction_identity_uq UNIQUE (id, buyer_user_id),
    CONSTRAINT c6_sanction_cycle_fk FOREIGN KEY (cycle_id, buyer_user_id)
        REFERENCES public.consignment_refusal_cycles (id, buyer_user_id) ON DELETE RESTRICT,
    CONSTRAINT c6_sanction_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c6_sanction_rule_ck CHECK (rule IN (
        'main_cod_returns', 'isekai_unpaid_carrier_returns')),
    CONSTRAINT c6_sanction_threshold_ck CHECK (threshold BETWEEN 2 AND 100),
    CONSTRAINT c6_sanction_window_ck CHECK (
        (rule = 'main_cod_returns' AND window_from IS NULL AND window_to IS NULL)
        OR (rule = 'isekai_unpaid_carrier_returns' AND window_from IS NOT NULL
            AND window_to IS NOT NULL AND window_to = t0 AND window_from = t0 - interval '2160 hours')
    ),
    CONSTRAINT c6_sanction_state_ck CHECK (state IN ('pending', 'applied', 'cycle_closed')),
    CONSTRAINT c6_sanction_version_ck CHECK (version BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c6_sanction_time_ck CHECK (isfinite(t0))
);
CREATE INDEX c6_sanction_pending_idx
    ON public.consignment_sanctions (buyer_user_id) WHERE state = 'pending';

CREATE TABLE public.consignment_sanction_members (
    sanction_id uuid NOT NULL CONSTRAINT c6_member_sanction_fk
        REFERENCES public.consignment_sanctions(id) ON DELETE RESTRICT,
    return_fact_id uuid NOT NULL CONSTRAINT c6_member_fact_fk
        REFERENCES public.consignment_return_facts(return_fact_id) ON DELETE RESTRICT,
    CONSTRAINT c6_member_pk PRIMARY KEY (sanction_id, return_fact_id),
    -- 构成项全局只用一次：一个返回事实不能凑数两个处罚。
    CONSTRAINT c6_member_fact_uq UNIQUE (return_fact_id)
);

CREATE FUNCTION public.c6_check_sanction_members() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    checked uuid;
    required smallint;
    present integer;
BEGIN
    IF TG_TABLE_NAME = 'consignment_sanctions' THEN
        checked := NEW.id;
    ELSE
        checked := NEW.sanction_id;
    END IF;
    SELECT sanction.threshold INTO required
    FROM public.consignment_sanctions AS sanction WHERE sanction.id = checked;
    SELECT count(*) INTO present
    FROM public.consignment_sanction_members AS member WHERE member.sanction_id = checked;
    -- 处罚、恰 threshold 个构成项、唯一账号工作在 COMMIT 前整体成形（触发返回事实必为构成项之一）。
    IF required IS NULL OR present <> required
       OR NOT EXISTS (
           SELECT 1 FROM public.consignment_sanctions AS sanction
           JOIN public.consignment_sanction_members AS member
             ON member.sanction_id = sanction.id
            AND member.return_fact_id = sanction.trigger_return_fact_id
           WHERE sanction.id = checked)
       OR NOT EXISTS (
           SELECT 1 FROM public.consignment_account_work AS work WHERE work.sanction_id = checked) THEN
        RAISE EXCEPTION '处罚必须同事务带齐构成项与唯一账号工作'
            USING ERRCODE = '23514', CONSTRAINT = 'c6_sanction_shape';
    END IF;
    RETURN NULL;
END
$function$;

CREATE TABLE public.consignment_account_work (
    id uuid NOT NULL CONSTRAINT c6_account_work_pk PRIMARY KEY,
    sanction_id uuid NOT NULL,
    buyer_user_id uuid NOT NULL,
    state text NOT NULL DEFAULT 'pending',
    version bigint NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CONSTRAINT c6_account_work_sanction_uq UNIQUE (sanction_id),
    CONSTRAINT c6_account_work_sanction_fk FOREIGN KEY (sanction_id, buyer_user_id)
        REFERENCES public.consignment_sanctions (id, buyer_user_id) ON DELETE RESTRICT,
    CONSTRAINT c6_account_work_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c6_account_work_state_ck CHECK (state IN ('pending', 'claimed', 'completed', 'cancelled')),
    CONSTRAINT c6_account_work_version_ck CHECK (version BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c6_account_work_time_ck CHECK (isfinite(created_at))
);
CREATE CONSTRAINT TRIGGER c6_sanction_shape
AFTER INSERT ON public.consignment_sanctions
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.c6_check_sanction_members();
CREATE CONSTRAINT TRIGGER c6_sanction_member_shape
AFTER INSERT ON public.consignment_sanction_members
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.c6_check_sanction_members();

-- 周期、处罚与账号工作是可变投影：只允许一步前进的状态迁移，身份列不可改。
CREATE FUNCTION public.c6_guard_projection_transition() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF TG_TABLE_NAME = 'consignment_refusal_cycles' THEN
        IF OLD.state <> 'open' OR NEW.state <> 'closed'
           OR NEW.id <> OLD.id OR NEW.buyer_user_id <> OLD.buyer_user_id
           OR NEW.ordinal <> OLD.ordinal OR NEW.opened_at <> OLD.opened_at THEN
            RAISE EXCEPTION '拒收周期只能由 open 关闭一次'
                USING ERRCODE = '23514', CONSTRAINT = 'c6_cycle_transition';
        END IF;
    ELSIF TG_TABLE_NAME = 'consignment_sanctions' THEN
        IF OLD.state <> 'pending' OR NEW.state NOT IN ('applied', 'cycle_closed')
           OR NEW.version <> OLD.version + 1
           OR ROW(NEW.id, NEW.buyer_user_id, NEW.cycle_id, NEW.rule, NEW.t0, NEW.threshold,
                  NEW.window_from, NEW.window_to, NEW.trigger_return_fact_id)
              IS DISTINCT FROM
              ROW(OLD.id, OLD.buyer_user_id, OLD.cycle_id, OLD.rule, OLD.t0, OLD.threshold,
                  OLD.window_from, OLD.window_to, OLD.trigger_return_fact_id) THEN
            RAISE EXCEPTION '处罚只能由 pending 前进一次且规则与构成不可改'
                USING ERRCODE = '23514', CONSTRAINT = 'c6_sanction_transition';
        END IF;
    ELSE
        IF NEW.version <> OLD.version + 1 OR NEW.id <> OLD.id
           OR NEW.sanction_id <> OLD.sanction_id OR NEW.buyer_user_id <> OLD.buyer_user_id
           OR NEW.created_at <> OLD.created_at
           OR NOT (
               (OLD.state = 'pending' AND NEW.state IN ('claimed', 'cancelled'))
               OR (OLD.state = 'claimed' AND NEW.state IN ('pending', 'completed', 'cancelled'))
           ) THEN
            RAISE EXCEPTION '账号工作状态迁移不合法'
                USING ERRCODE = '23514', CONSTRAINT = 'c6_account_work_transition';
        END IF;
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c6_cycle_transition
BEFORE UPDATE ON public.consignment_refusal_cycles
FOR EACH ROW EXECUTE FUNCTION public.c6_guard_projection_transition();
CREATE TRIGGER c6_sanction_transition
BEFORE UPDATE ON public.consignment_sanctions
FOR EACH ROW EXECUTE FUNCTION public.c6_guard_projection_transition();
CREATE TRIGGER c6_account_work_transition
BEFORE UPDATE ON public.consignment_account_work
FOR EACH ROW EXECUTE FUNCTION public.c6_guard_projection_transition();

-- 新购阻断：买家有 pending 处罚时不得新建 Sale（与应用层的买家互斥互为兜底，不变量 2）。
CREATE FUNCTION public.c6_reject_sale_with_pending_sanction() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF EXISTS (
        SELECT 1 FROM public.consignment_sanctions AS sanction
        WHERE sanction.buyer_user_id = NEW.buyer_user_id AND sanction.state = 'pending'
    ) THEN
        RAISE EXCEPTION '买家有待执行处罚，不能新购'
            USING ERRCODE = '23514', CONSTRAINT = 'c6_sale_buyer_sanction_pending';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c6_sale_buyer_sanction
BEFORE INSERT ON public.consignment_sales
FOR EACH ROW EXECUTE FUNCTION public.c6_reject_sale_with_pending_sanction();

-- ⑧ 季末关闭未起写的取得工作（6D6）：只追加，且该工作确无起写事实。
CREATE TABLE public.consignment_work_closures (
    work_id uuid NOT NULL CONSTRAINT c6_closure_pk PRIMARY KEY
        CONSTRAINT c6_closure_work_fk REFERENCES public.mail_disposition_work(id) ON DELETE RESTRICT,
    position_id uuid NOT NULL CONSTRAINT c6_closure_position_fk
        REFERENCES public.consignment_custody_positions(id) ON DELETE RESTRICT,
    reason text NOT NULL,
    closed_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CONSTRAINT c6_closure_reason_ck CHECK (reason = 'season_ended'),
    CONSTRAINT c6_closure_time_ck CHECK (isfinite(closed_at))
);
CREATE FUNCTION public.c6_guard_work_closure() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    -- 已起写的工作不可关闭：起写后只能核对，不能终结（不变量 4）。
    IF EXISTS (SELECT 1 FROM public.mail_disposition_write_starts AS start
               WHERE start.work_id = NEW.work_id) THEN
        RAISE EXCEPTION '已起写的取得工作不能关闭'
            USING ERRCODE = '23514', CONSTRAINT = 'c6_closure_started_work';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c6_closure_binding
BEFORE INSERT ON public.consignment_work_closures
FOR EACH ROW EXECUTE FUNCTION public.c6_guard_work_closure();


-- ⑨ 资金：寄售合同分配键、来源种类与收款事实。
-- 分配唯一键：账户用途两 FK 空，拍卖只带 lot，寄售只带合同；两个可空列均按 NULLS NOT DISTINCT 参与。
ALTER TABLE public.settlement_funding_allocations
    ADD COLUMN consignment_contract_id uuid CONSTRAINT c6_allocation_contract_fk
        REFERENCES public.consignment_contracts(contract_id) ON DELETE RESTRICT;
ALTER TABLE public.settlement_funding_allocations
    DROP CONSTRAINT settlement_funding_allocations_uid_purpose_lot_id_key,
    DROP CONSTRAINT settlement_funding_allocations_purpose_check,
    ADD CONSTRAINT c6_allocation_key_uq
        UNIQUE NULLS NOT DISTINCT (uid, purpose, lot_id, consignment_contract_id),
    ADD CONSTRAINT c6_allocation_owner_ck CHECK (
        lot_id IS NULL OR consignment_contract_id IS NULL
    ),
    ADD CONSTRAINT c6_allocation_purpose_ck CHECK (purpose IN (
        'proprietary', 'compensation', 'quarantine', 'seller', 'tax', 'retained', 'refund',
        'carrier', 'consignment_transit', 'fee', 'carrier_writeoff'
    )),
    -- 过渡、手续费与不可支用核销子账只属于某个寄售合同；寄售合同行只用下列用途。
    ADD CONSTRAINT c6_allocation_contract_purpose_ck CHECK (
        (consignment_contract_id IS NULL
            AND purpose NOT IN ('consignment_transit', 'fee', 'carrier_writeoff'))
        OR (consignment_contract_id IS NOT NULL AND purpose IN (
            'seller', 'fee', 'refund', 'carrier', 'retained', 'consignment_transit',
            'carrier_writeoff'))
    );

ALTER TABLE public.settlement_funding_entries
    DROP CONSTRAINT settlement_funding_entries_kind_check,
    ADD CONSTRAINT c6_entry_kind_ck CHECK (kind IN (
        'issue', 'transfer', 'reserve', 'in_flight', 'unknown', 'spent', 'return', 'claimed'
    ));

-- 寄售的 record_only 载体 1-credit 来源：只绑定寄售 isekai 付款载体义务在 main 的冻结派发轮，
-- 且发行目标必须是同一合同、同一义务的 carrier 分配；账户与成交都冻结为 record_only 且 UID 一致。
CREATE FUNCTION public.settlement_record_carrier_consignment_valid_000136(
    checked_source_id uuid,
    checked_credit_id uuid
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
        JOIN public.consignment_dispatch_rounds AS round ON round.delivery_id = delivery.id
        JOIN public.settlement_obligations AS obligation
          ON obligation.id = round.obligation_id AND obligation.contract_id = round.contract_id
        JOIN public.settlement_obligation_fulfillments AS fulfillment
          ON fulfillment.obligation_id = obligation.id
        JOIN public.consignment_sales AS sale ON sale.contract_id = obligation.contract_id
        JOIN public.settlement_funding_allocations AS credit
          ON credit.id = checked_credit_id
         AND credit.uid = source.uid
         AND credit.purpose = 'carrier'
         AND credit.consignment_contract_id = obligation.contract_id
         AND credit.obligation_id = obligation.id
        WHERE source.id = checked_source_id
          AND source.kind = 'record_carrier'
          AND source.amount = 1
          AND account.accounting_mode = 'record_only'
          AND sale.funding_uid = source.uid
          AND sale.funding_mode = 'record_only'
          AND obligation.obligation_kind = 'winner_payment_carrier_cod'
          AND obligation.world_id = 'isekai'
          AND obligation.carrier_credit = 1
          AND obligation.business_state = 'ready'
          AND fulfillment.dispatch_world = 'main'
          AND fulfillment.delivery_operation = 'send'
          AND round.world_id = 'main'
          AND round.credits = 1
          AND round.cod = obligation.amount::numeric + obligation.carrier_credit::numeric
          AND delivery.origin_kind = 'settlement'
          AND delivery.origin_id = obligation.id::text
          AND delivery.payload->>'operation' = 'send'
          AND delivery.payload->>'world' = 'main'
          AND delivery.payload->>'credits' = '1'
          AND delivery.payload->>'cod' =
              (obligation.amount::numeric + obligation.carrier_credit::numeric)::text
          AND delivery.payload->'equipment_secret_refs' = '[]'::jsonb
    );
$function$;

-- 现行累计 issue 上限触发器（000131 版）整函数复制，只新增跨 source 守卫：
-- 贷入寄售合同分配的 issue 只能来自绑定了寄售收据的付款 source（上限即收据新发行额 U）或 record_carrier，
-- opening（reconciliation）不得直接向合同用途发行，只能经 transfer 借出已含部分。
CREATE OR REPLACE FUNCTION public.enforce_settlement_funding_issue_limit_000120()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $function$
DECLARE
    source_uid text;
    source_kind text;
    source_key uuid;
    source_amount bigint;
    source_proof_digest bytea;
    counted_source uuid;
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
            AND (public.settlement_record_carrier_source_valid_000120(NEW.source_id, true)
                 OR public.settlement_record_carrier_consignment_valid_000136(
                        NEW.source_id, NEW.credit_id)))
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
    -- 000136 新增：寄售合同分配只接受绑定了收据的付款 source 或 record_carrier 的发行。
    IF NEW.kind = 'issue' AND EXISTS (
        SELECT 1 FROM public.settlement_funding_allocations AS credit
        WHERE credit.id = NEW.credit_id AND credit.consignment_contract_id IS NOT NULL
    ) AND NOT (
        (source_kind = 'collected_payment' AND EXISTS (
            SELECT 1 FROM public.consignment_cod_receipts AS receipt
            WHERE receipt.source_id = NEW.source_id AND receipt.new_issue = source_amount))
        OR source_kind = 'record_carrier'
    ) THEN
        RAISE EXCEPTION '寄售合同分配只能由绑定收据的付款来源发行'
            USING ERRCODE = '23514', CONSTRAINT = 'c6_issue_cross_source';
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
    UPDATE public.settlement_funding_sources AS source
    SET issued_amount = source.issued_amount + NEW.amount
    WHERE source.id = NEW.source_id
      AND source.uid = NEW.uid
      AND source.issued_amount::numeric + NEW.amount::numeric <= source.amount::numeric
    RETURNING source.id INTO counted_source;
    IF counted_source IS NULL THEN
        RAISE EXCEPTION '资金来源分录超过冻结额度'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$function$;

-- COD 实收：一笔远端实收只有一份经济结果。主键仍是 source_id；经济键（原 UID、世界、原付款 Sent MID、
-- COD 腿）、付款义务、付款 delivery 各自唯一；单存 G、A、U，0 <= A <= G 且 U = G - A。
CREATE TABLE public.consignment_cod_receipts (
    source_id uuid NOT NULL CONSTRAINT c6_cod_receipt_pk PRIMARY KEY,
    uid text NOT NULL,
    contract_id uuid NOT NULL,
    payment_obligation_id uuid NOT NULL,
    delivery_id uuid NOT NULL,
    world_id text NOT NULL,
    sent_mid text NOT NULL,
    sent_fence bigint NOT NULL,
    cod_leg text NOT NULL,
    remote_payment_ref text,
    receipt_key_id uuid NOT NULL,
    gross bigint NOT NULL,
    opening_included bigint NOT NULL,
    new_issue bigint NOT NULL,
    CONSTRAINT c6_cod_receipt_key_uq UNIQUE (uid, world_id, sent_mid, cod_leg),
    CONSTRAINT c6_cod_receipt_key_id_uq UNIQUE (receipt_key_id),
    CONSTRAINT c6_cod_receipt_obligation_uq UNIQUE (payment_obligation_id),
    CONSTRAINT c6_cod_receipt_delivery_uq UNIQUE (delivery_id),
    CONSTRAINT c6_cod_receipt_contract_uq UNIQUE (source_id, contract_id),
    CONSTRAINT c6_cod_receipt_key_identity_uq UNIQUE (source_id, receipt_key_id),
    CONSTRAINT c6_cod_receipt_base_fk
        FOREIGN KEY (source_id, uid, payment_obligation_id, delivery_id)
        REFERENCES public.settlement_funding_receipts (source_id, uid, payment_obligation_id, delivery_id)
        ON DELETE RESTRICT,
    CONSTRAINT c6_cod_receipt_obligation_fk FOREIGN KEY (payment_obligation_id, contract_id)
        REFERENCES public.settlement_obligations (id, contract_id) ON DELETE RESTRICT,
    CONSTRAINT c6_cod_receipt_sent_fk FOREIGN KEY (world_id, sent_mid, delivery_id, sent_fence)
        REFERENCES public.delivery_sent_bindings (world_id, external_mail_id, delivery_id, fencing_version)
        ON DELETE RESTRICT,
    CONSTRAINT c6_cod_receipt_world_ck CHECK (world_id = 'main'),
    CONSTRAINT c6_cod_receipt_uid_ck CHECK (uid COLLATE "C" ~ '^[1-9][0-9]*$' AND length(uid) <= 19),
    CONSTRAINT c6_cod_receipt_leg_ck CHECK (cod_leg IN ('original', 'carrier')),
    CONSTRAINT c6_cod_receipt_ref_ck CHECK (
        remote_payment_ref IS NULL OR public.consignment_safe_observation_text(remote_payment_ref, 128)),
    -- 金额守恒：0 <= A <= G，U = G - A；付款 source 的可发行上限是 U，可为零。
    CONSTRAINT c6_cod_receipt_split_ck CHECK (
        gross > 0 AND opening_included >= 0 AND opening_included <= gross
        AND new_issue = gross - opening_included
    )
);
CREATE UNIQUE INDEX c6_cod_receipt_remote_ref_uq
    ON public.consignment_cod_receipts (uid, remote_payment_ref) WHERE remote_payment_ref IS NOT NULL;


-- opening 份额：对账 owner 以不可变份额指明原 UID、边界、opening source、实际 opening issue entry、
-- 原 allocation 与可归属金额区间；同一 entry 的区间不相交（触发器串行化后判定），每段只被一个实收业务键消费一次。
CREATE TABLE public.consignment_opening_slices (
    id uuid NOT NULL CONSTRAINT c6_slice_pk PRIMARY KEY,
    uid text NOT NULL,
    receipt_key_id uuid NOT NULL,
    boundary_at timestamptz NOT NULL,
    opening_source_id uuid NOT NULL CONSTRAINT c6_slice_source_fk
        REFERENCES public.settlement_funding_sources(id) ON DELETE RESTRICT,
    opening_issue_entry_id uuid NOT NULL CONSTRAINT c6_slice_entry_fk
        REFERENCES public.settlement_funding_entries(id) ON DELETE RESTRICT,
    original_allocation_id uuid NOT NULL CONSTRAINT c6_slice_allocation_fk
        REFERENCES public.settlement_funding_allocations(id) ON DELETE RESTRICT,
    amount_from bigint NOT NULL,
    amount_to bigint NOT NULL,
    basis text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    CONSTRAINT c6_slice_key_uq UNIQUE (id, receipt_key_id),
    CONSTRAINT c6_slice_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c6_slice_uid_ck CHECK (uid COLLATE "C" ~ '^[1-9][0-9]*$' AND length(uid) <= 19),
    CONSTRAINT c6_slice_range_ck CHECK (amount_from >= 0 AND amount_to > amount_from),
    CONSTRAINT c6_slice_basis_ck CHECK (public.consignment_safe_observation_text(basis, 256)),
    CONSTRAINT c6_slice_time_ck CHECK (isfinite(boundary_at) AND isfinite(created_at))
);
CREATE INDEX c6_slice_entry_idx
    ON public.consignment_opening_slices (opening_issue_entry_id, amount_from);

-- SECURITY DEFINER：运行账号对 settlement_funding_entries 只有 SELECT、INSERT，没有 UPDATE，
-- 而本触发器要 FOR UPDATE 锁 entry 行让同一 entry 的并发登记串行化（6B2 PG 例发现，未发布迁移原地修改）。
CREATE FUNCTION public.c6_guard_opening_slice() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog
AS $function$
DECLARE
    entry_row public.settlement_funding_entries%ROWTYPE;
BEGIN
    -- 对 entry 行加锁，让同一 entry 上的并发登记串行化，再判区间相交（btree_gist 不作依赖）。
    SELECT * INTO entry_row
    FROM public.settlement_funding_entries WHERE id = NEW.opening_issue_entry_id FOR UPDATE;
    IF NOT FOUND OR entry_row.kind <> 'issue' OR entry_row.uid <> NEW.uid
       OR entry_row.source_id IS DISTINCT FROM NEW.opening_source_id
       OR entry_row.credit_id IS DISTINCT FROM NEW.original_allocation_id
       OR NEW.amount_to > entry_row.amount THEN
        RAISE EXCEPTION '份额必须落在同 UID opening 发行分录的正额范围内'
            USING ERRCODE = '23514', CONSTRAINT = 'c6_slice_entry_range';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM public.settlement_funding_sources AS source
        WHERE source.id = NEW.opening_source_id AND source.uid = NEW.uid
          AND source.kind IN ('reconciliation', 'record_opening')
    ) THEN
        RAISE EXCEPTION '份额只能来自本 UID 的 opening 来源'
            USING ERRCODE = '23514', CONSTRAINT = 'c6_slice_source_kind';
    END IF;
    IF EXISTS (
        SELECT 1 FROM public.consignment_opening_slices AS other
        WHERE other.opening_issue_entry_id = NEW.opening_issue_entry_id
          AND other.amount_from < NEW.amount_to AND NEW.amount_from < other.amount_to
    ) THEN
        RAISE EXCEPTION '同一 opening 分录的份额区间不得相交'
            USING ERRCODE = '23514', CONSTRAINT = 'c6_slice_overlap';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c6_slice_binding
BEFORE INSERT ON public.consignment_opening_slices
FOR EACH ROW EXECUTE FUNCTION public.c6_guard_opening_slice();

CREATE TABLE public.consignment_opening_slice_uses (
    slice_id uuid NOT NULL CONSTRAINT c6_slice_use_pk PRIMARY KEY,
    receipt_source_id uuid NOT NULL,
    receipt_key_id uuid NOT NULL,
    CONSTRAINT c6_slice_use_slice_fk FOREIGN KEY (slice_id, receipt_key_id)
        REFERENCES public.consignment_opening_slices (id, receipt_key_id) ON DELETE RESTRICT,
    CONSTRAINT c6_slice_use_receipt_fk FOREIGN KEY (receipt_source_id, receipt_key_id)
        REFERENCES public.consignment_cod_receipts (source_id, receipt_key_id) ON DELETE RESTRICT
);

-- 三条延迟校验（COMMIT 前）：重分类合计 = A、新发行合计 = U、最终用途合计 = G；
-- 付款 source 的额度 = U。任何一项不等整笔回滚，不留孤立 satisfied 或半笔收入。
CREATE FUNCTION public.c6_check_cod_receipt_balance() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    receipt public.consignment_cod_receipts%ROWTYPE;
    source_amount bigint;
    reclassified numeric;
    issued numeric;
    transit_credit numeric;
    transit_debit numeric;
    transit_id uuid;
BEGIN
    SELECT * INTO receipt FROM public.consignment_cod_receipts WHERE source_id = NEW.source_id;
    SELECT source.amount INTO source_amount
    FROM public.settlement_funding_sources AS source
    WHERE source.id = receipt.source_id AND source.uid = receipt.uid
      AND source.kind = 'collected_payment';
    SELECT COALESCE(sum(slice.amount_to - slice.amount_from), 0) INTO reclassified
    FROM public.consignment_opening_slice_uses AS use
    JOIN public.consignment_opening_slices AS slice ON slice.id = use.slice_id
    WHERE use.receipt_source_id = receipt.source_id;
    SELECT COALESCE(sum(entry.amount), 0) INTO issued
    FROM public.settlement_funding_entries AS entry
    WHERE entry.source_id = receipt.source_id AND entry.kind = 'issue';
    SELECT allocation.id INTO transit_id
    FROM public.settlement_funding_allocations AS allocation
    WHERE allocation.uid = receipt.uid AND allocation.purpose = 'consignment_transit'
      AND allocation.lot_id IS NULL AND allocation.consignment_contract_id = receipt.contract_id;
    SELECT COALESCE(sum(entry.amount) FILTER (WHERE entry.credit_id = transit_id), 0),
           COALESCE(sum(entry.amount) FILTER (WHERE entry.debit_id = transit_id), 0)
    INTO transit_credit, transit_debit
    FROM public.settlement_funding_entries AS entry
    WHERE transit_id IS NOT NULL AND (entry.credit_id = transit_id OR entry.debit_id = transit_id);
    IF source_amount IS DISTINCT FROM receipt.new_issue
       OR reclassified <> receipt.opening_included
       OR issued <> receipt.new_issue
       OR transit_id IS NULL
       OR transit_credit <> receipt.gross
       OR transit_debit <> receipt.gross THEN
        RAISE EXCEPTION '实收分账必须满足 重分类合计=A、新发行合计=U、最终用途合计=G'
            USING ERRCODE = '23514', CONSTRAINT = 'c6_cod_receipt_balance';
    END IF;
    RETURN NULL;
END
$function$;
CREATE CONSTRAINT TRIGGER c6_cod_receipt_balance
AFTER INSERT ON public.consignment_cod_receipts
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.c6_check_cod_receipt_balance();

-- 费用事实：F>0 时 funded 恰一条 earned，全额退款恰一条 reversed；F=0 全无行（不占位）。
CREATE TABLE public.consignment_fee_facts (
    id uuid NOT NULL CONSTRAINT c6_fee_pk PRIMARY KEY,
    contract_id uuid NOT NULL,
    kind text NOT NULL,
    amount bigint NOT NULL,
    receipt_source_id uuid NOT NULL,
    fee_entry_id uuid NOT NULL CONSTRAINT c6_fee_entry_fk
        REFERENCES public.settlement_funding_entries(id) ON DELETE RESTRICT,
    earned_fact_id uuid,
    refund_obligation_id uuid,
    CONSTRAINT c6_fee_kind_uq UNIQUE (contract_id, kind),
    CONSTRAINT c6_fee_identity_uq UNIQUE (id, contract_id, amount),
    CONSTRAINT c6_fee_receipt_fk FOREIGN KEY (receipt_source_id, contract_id)
        REFERENCES public.consignment_cod_receipts (source_id, contract_id) ON DELETE RESTRICT,
    CONSTRAINT c6_fee_earned_fk FOREIGN KEY (earned_fact_id, contract_id, amount)
        REFERENCES public.consignment_fee_facts (id, contract_id, amount) ON DELETE RESTRICT,
    CONSTRAINT c6_fee_refund_fk FOREIGN KEY (refund_obligation_id, contract_id)
        REFERENCES public.settlement_obligations (id, contract_id) ON DELETE RESTRICT,
    CONSTRAINT c6_fee_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c6_fee_amount_ck CHECK (amount > 0),
    CONSTRAINT c6_fee_shape_ck CHECK (
        (kind = 'earned' AND earned_fact_id IS NULL AND refund_obligation_id IS NULL)
        OR (kind = 'reversed' AND earned_fact_id IS NOT NULL AND refund_obligation_id IS NOT NULL)
    )
);

-- 本金结论：每 (合同, 载体义务, 原 delivery, 轮) 至多一个，付款本金与 RTS 本金互斥（同一行只有一种）。
CREATE TABLE public.consignment_principal_facts (
    id uuid NOT NULL CONSTRAINT c6_principal_pk PRIMARY KEY,
    contract_id uuid NOT NULL,
    carrier_obligation_id uuid NOT NULL,
    original_delivery_id uuid NOT NULL CONSTRAINT c6_principal_delivery_fk
        REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    round integer NOT NULL,
    conclusion text NOT NULL,
    mode text NOT NULL,
    amount bigint NOT NULL,
    CONSTRAINT c6_principal_once_uq
        UNIQUE (contract_id, carrier_obligation_id, original_delivery_id, round),
    CONSTRAINT c6_principal_obligation_fk FOREIGN KEY (carrier_obligation_id, contract_id)
        REFERENCES public.settlement_obligations (id, contract_id) ON DELETE RESTRICT,
    CONSTRAINT c6_principal_round_fk FOREIGN KEY (contract_id, carrier_obligation_id, round, original_delivery_id)
        REFERENCES public.consignment_dispatch_rounds (contract_id, obligation_id, round, delivery_id)
        ON DELETE RESTRICT,
    CONSTRAINT c6_principal_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c6_principal_conclusion_ck CHECK (conclusion IN ('payment_principal', 'rts_principal')),
    CONSTRAINT c6_principal_mode_ck CHECK (mode IN ('strict_cash', 'record_only')),
    CONSTRAINT c6_principal_amount_ck CHECK (amount = 1)
);

-- 现金退件的 UID 级 opening 阻断：最迟随现金 Take 的 write_started 同事务建立；
-- 只有经原资金消费（consignment_funding_returns）同事务才能解除。
CREATE TABLE public.consignment_cash_return_blocks (
    id uuid NOT NULL CONSTRAINT c6_block_pk PRIMARY KEY,
    uid text NOT NULL CONSTRAINT c6_block_account_fk
        REFERENCES public.settlement_funding_accounts(uid) ON DELETE RESTRICT,
    intake_id uuid NOT NULL CONSTRAINT c6_block_intake_fk
        REFERENCES public.consignment_return_intakes(id) ON DELETE RESTRICT,
    leg text NOT NULL,
    original_delivery_id uuid NOT NULL CONSTRAINT c6_block_delivery_fk
        REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    amount bigint NOT NULL,
    original_mode text NOT NULL,
    state text NOT NULL DEFAULT 'open',
    opened_in_write_start uuid NOT NULL CONSTRAINT c6_block_start_fk
        REFERENCES public.mail_disposition_write_starts(work_id) ON DELETE RESTRICT,
    opened_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    cleared_by uuid,
    cleared_at timestamptz,
    CONSTRAINT c6_block_leg_uq UNIQUE (intake_id, leg),
    CONSTRAINT c6_block_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c6_block_leg_ck CHECK (leg IN ('cash_refund', 'cash_payout', 'cash_principal')),
    CONSTRAINT c6_block_amount_ck CHECK (amount > 0),
    CONSTRAINT c6_block_mode_ck CHECK (original_mode IN ('strict_cash', 'record_only')),
    CONSTRAINT c6_block_state_ck CHECK (
        (state = 'open' AND cleared_by IS NULL AND cleared_at IS NULL)
        OR (state = 'cleared' AND cleared_by IS NOT NULL AND cleared_at IS NOT NULL)
    ),
    CONSTRAINT c6_block_time_ck CHECK (isfinite(opened_at))
);
CREATE INDEX c6_block_open_idx ON public.consignment_cash_return_blocks (uid) WHERE state = 'open';

-- 原资金消费：一笔 returned source 回原 UID / 合同 / 用途，金额等于原支出；每个旧 delivery 一次。
CREATE TABLE public.consignment_funding_returns (
    id uuid NOT NULL CONSTRAINT c6_funding_return_pk PRIMARY KEY,
    uid text NOT NULL,
    contract_id uuid NOT NULL,
    intake_id uuid NOT NULL,
    consumption_id uuid NOT NULL,
    leg text NOT NULL,
    source_id uuid NOT NULL,
    original_delivery_id uuid NOT NULL CONSTRAINT c6_funding_return_delivery_fk
        REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    original_spent_entry_id uuid NOT NULL CONSTRAINT c6_funding_return_spent_fk
        REFERENCES public.settlement_funding_entries(id) ON DELETE RESTRICT,
    amount bigint NOT NULL,
    CONSTRAINT c6_funding_return_source_uq UNIQUE (source_id),
    CONSTRAINT c6_funding_return_consumption_uq UNIQUE (consumption_id),
    CONSTRAINT c6_funding_return_delivery_uq UNIQUE (original_delivery_id),
    CONSTRAINT c6_funding_return_leg_uq UNIQUE (intake_id, leg),
    CONSTRAINT c6_funding_return_identity_uq UNIQUE (id, intake_id, leg),
    CONSTRAINT c6_funding_return_consumption_fk FOREIGN KEY (consumption_id, intake_id, contract_id, leg)
        REFERENCES public.consignment_return_consumptions (id, intake_id, contract_id, leg)
        ON DELETE RESTRICT,
    CONSTRAINT c6_funding_return_source_fk FOREIGN KEY (uid, source_id)
        REFERENCES public.settlement_funding_sources (uid, id) ON DELETE RESTRICT,
    CONSTRAINT c6_funding_return_id_ck CHECK (public.c4_valid_uuid(id)),
    CONSTRAINT c6_funding_return_leg_ck CHECK (leg IN (
        'cash_refund', 'cash_payout', 'cash_principal')),
    CONSTRAINT c6_funding_return_amount_ck CHECK (amount > 0)
);
ALTER TABLE public.consignment_cash_return_blocks
    ADD CONSTRAINT c6_block_cleared_by_fk FOREIGN KEY (cleared_by, intake_id, leg)
        REFERENCES public.consignment_funding_returns (id, intake_id, leg) ON DELETE RESTRICT;

CREATE FUNCTION public.c6_guard_cash_block_transition() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    -- 阻断只能 open -> cleared 一次，且清除时同事务必须写入对应的原资金消费；
    -- 只拿到 Take 证明、只消费邮件、仅有处罚事实都不能解除。
    IF OLD.state <> 'open' OR NEW.state <> 'cleared'
       OR ROW(NEW.id, NEW.uid, NEW.intake_id, NEW.leg, NEW.original_delivery_id, NEW.amount,
              NEW.original_mode, NEW.opened_in_write_start, NEW.opened_at)
          IS DISTINCT FROM
          ROW(OLD.id, OLD.uid, OLD.intake_id, OLD.leg, OLD.original_delivery_id, OLD.amount,
              OLD.original_mode, OLD.opened_in_write_start, OLD.opened_at)
       OR NOT EXISTS (
           SELECT 1 FROM public.consignment_funding_returns AS funding_return
           WHERE funding_return.id = NEW.cleared_by
             AND funding_return.intake_id = OLD.intake_id AND funding_return.leg = OLD.leg
             AND funding_return.uid = OLD.uid AND funding_return.amount = OLD.amount
             AND funding_return.original_delivery_id = OLD.original_delivery_id
             AND funding_return.xmin = pg_current_xact_id()::xid
       ) THEN
        RAISE EXCEPTION '现金退件阻断只能随同事务的原资金消费解除'
            USING ERRCODE = '23514', CONSTRAINT = 'c6_block_transition';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c6_block_transition
BEFORE UPDATE ON public.consignment_cash_return_blocks
FOR EACH ROW EXECUTE FUNCTION public.c6_guard_cash_block_transition();

-- 同 UID 存在未核清的现金退件阻断时，不得建立 opening 来源（含零额建界与调整、结转）。
CREATE FUNCTION public.c6_reject_opening_during_cash_block() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF EXISTS (
        SELECT 1 FROM public.consignment_cash_return_blocks AS block
        WHERE block.uid = NEW.uid AND block.state = 'open'
    ) THEN
        RAISE EXCEPTION '该 UID 有未核清的现金退件，不能建立 opening'
            USING ERRCODE = '23514', CONSTRAINT = 'c6_opening_blocked_by_cash_return';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c6_opening_cash_block
BEFORE INSERT ON public.settlement_funding_sources
FOR EACH ROW WHEN (NEW.kind IN ('reconciliation', 'record_opening'))
EXECUTE FUNCTION public.c6_reject_opening_during_cash_block();

-- funded 投影：一份实收一个合同；F>0 恰一条 earned、F=0 零条（按冻结 F 判行数）。
CREATE TABLE public.consignment_funded_sales (
    contract_id uuid NOT NULL CONSTRAINT c6_funded_pk PRIMARY KEY
        CONSTRAINT c6_funded_sale_fk REFERENCES public.consignment_sales(contract_id) ON DELETE RESTRICT,
    uid text NOT NULL,
    payment_obligation_id uuid NOT NULL,
    receipt_source_id uuid NOT NULL,
    disposition text NOT NULL DEFAULT 'funded',
    version bigint NOT NULL DEFAULT 1,
    CONSTRAINT c6_funded_obligation_uq UNIQUE (payment_obligation_id),
    CONSTRAINT c6_funded_receipt_uq UNIQUE (receipt_source_id),
    CONSTRAINT c6_funded_receipt_fk FOREIGN KEY (receipt_source_id, contract_id)
        REFERENCES public.consignment_cod_receipts (source_id, contract_id) ON DELETE RESTRICT,
    CONSTRAINT c6_funded_disposition_ck CHECK (disposition IN (
        'funded', 'original_claimed', 'refund_converted')),
    CONSTRAINT c6_funded_version_ck CHECK (version BETWEEN 1 AND 9007199254740991)
);

CREATE FUNCTION public.c6_guard_funded_transition() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NEW.version <> OLD.version + 1
       OR ROW(NEW.contract_id, NEW.uid, NEW.payment_obligation_id, NEW.receipt_source_id)
          IS DISTINCT FROM
          ROW(OLD.contract_id, OLD.uid, OLD.payment_obligation_id, OLD.receipt_source_id)
       OR OLD.disposition <> 'funded'
       OR NEW.disposition NOT IN ('original_claimed', 'refund_converted') THEN
        RAISE EXCEPTION 'funded 投影只能前进一步且身份不可改'
            USING ERRCODE = '23514', CONSTRAINT = 'c6_funded_transition';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c6_funded_transition
BEFORE UPDATE ON public.consignment_funded_sales
FOR EACH ROW EXECUTE FUNCTION public.c6_guard_funded_transition();

CREATE FUNCTION public.c6_check_funded_fee_rows() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    frozen_fee bigint;
    earned integer;
BEGIN
    SELECT sale.fee INTO frozen_fee
    FROM public.consignment_sales AS sale WHERE sale.contract_id = NEW.contract_id;
    SELECT count(*) INTO earned
    FROM public.consignment_fee_facts AS fact
    WHERE fact.contract_id = NEW.contract_id AND fact.kind = 'earned' AND fact.amount = frozen_fee;
    IF earned <> (CASE WHEN frozen_fee > 0 THEN 1 ELSE 0 END)
       OR EXISTS (SELECT 1 FROM public.consignment_fee_facts AS fact
                  WHERE fact.contract_id = NEW.contract_id AND fact.amount <> frozen_fee) THEN
        RAISE EXCEPTION 'funded 的手续费行数必须与冻结 F 一致（F>0 恰一条 earned，F=0 零条）'
            USING ERRCODE = '23514', CONSTRAINT = 'c6_funded_fee_rows';
    END IF;
    RETURN NULL;
END
$function$;
CREATE CONSTRAINT TRIGGER c6_funded_fee_rows
AFTER INSERT ON public.consignment_funded_sales
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.c6_check_funded_fee_rows();


-- ⑩ Sale 的资金边界冻结（R5-C9）：最近一次 opening 来源（record_only 无现金 opening 时为空）与账户版本，
-- 让 certify_cod 能复验“原 Send 严格晚于原可认证开户边界”。合同表只追加，故只能随新行写入；
-- 生产在 000136 之前不可能有 Sale 行（购买能力恒假），SET NOT NULL 在非空表上失败即说明环境不符。
ALTER TABLE public.consignment_sales
    ADD COLUMN funding_boundary_source_id uuid CONSTRAINT c6_sale_boundary_source_fk
        REFERENCES public.settlement_funding_sources(id) ON DELETE RESTRICT,
    ADD COLUMN funding_account_version bigint;
ALTER TABLE public.consignment_sales
    ALTER COLUMN funding_account_version SET NOT NULL,
    ADD CONSTRAINT c6_sale_account_version_ck CHECK (funding_account_version >= 0);


-- ⑪ 命令账本：新增操作、目标种类、结果码与回执形状（回执校验函数整函数替换，旧分支原样保留）。
CREATE FUNCTION public.c6_valid_json_amount(value jsonb) RETURNS boolean
LANGUAGE sql IMMUTABLE PARALLEL SAFE SET search_path = pg_catalog
AS $function$
    SELECT CASE
        WHEN jsonb_typeof(value) = 'number'
             AND value::text COLLATE "C" ~ '^(0|[1-9][0-9]{0,15})$'
            THEN (value::text)::bigint <= 9007199254740991
        ELSE false
    END
$function$;

CREATE FUNCTION public.c6_valid_json_positive(value jsonb) RETURNS boolean
LANGUAGE sql IMMUTABLE PARALLEL SAFE SET search_path = pg_catalog
AS $function$
    SELECT CASE
        WHEN public.c6_valid_json_amount(value) THEN (value::text)::bigint >= 1
        ELSE false
    END
$function$;

-- 实收三元组：毛额 G 为正，G = A + U，0 <= A <= G。
CREATE FUNCTION public.c6_valid_json_split(gross jsonb, included jsonb, issued jsonb)
RETURNS boolean
LANGUAGE sql IMMUTABLE PARALLEL SAFE SET search_path = pg_catalog
AS $function$
    SELECT CASE
        WHEN public.c6_valid_json_positive(gross)
             AND public.c6_valid_json_amount(included)
             AND public.c6_valid_json_amount(issued)
            THEN (gross::text)::bigint = (included::text)::bigint + (issued::text)::bigint
        ELSE false
    END
$function$;

CREATE OR REPLACE FUNCTION public.c4_valid_v2_receipt(
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
            (status = 403 AND body->>'code' IN (
                'participant_ineligible', 'bot_party', 'action_not_allowed', 'self_purchase'))
            OR (status = 404 AND body->>'code' IN ('consignment_disabled', 'listing_not_found'))
            OR (status = 409 AND body->>'code' IN (
                'stale_version', 'listing_not_active', 'equipment_busy', 'equipment_committed',
                'custody_unproven', 'operations_hold', 'season_closed', 'hold_mismatch',
                'source_mismatch', 'evidence_unproven', 'evidence_conflict',
                'disposition_conflict', 'fence_mismatch', 'price_changed', 'funding_boundary',
                'opposite_payment_open', 'lease_lost'))
            OR (status = 429 AND body->>'code' = 'listing_limit_reached')
            OR (status = 503 AND body->>'code' IN ('mail_unavailable', 'funding_unavailable'))
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
        WHEN 'purchase' THEN
            RETURN status = 201 AND disposition = 'purchased'
                AND public.c4_valid_json_uuid(body->'listing_id')
                AND public.c4_valid_json_version(body->'listing_version')
                AND body->>'state' = 'sale_pending'
                AND public.c4_valid_json_uuid(body->'contract_id')
                AND public.c4_valid_json_version(body->'contract_version')
                AND public.c4_valid_json_uuid(body->'equipment_id')
                AND body->>'world' IN ('main', 'isekai')
                AND public.c6_valid_json_positive(body->'price')
                AND body->>'order_state' = 'awaiting_payment';
        WHEN 'reprice' THEN
            RETURN status = 200 AND disposition IN ('repriced', 'unchanged')
                AND public.c4_valid_json_uuid(body->'listing_id')
                AND public.c4_valid_json_version(body->'listing_version')
                AND body->>'state' = 'available'
                AND public.c6_valid_json_positive(body->'price');
        WHEN 'authorize_dispatch' THEN
            RETURN status = 200 AND disposition = 'round_created'
                AND public.c4_valid_json_uuid(body->'round_id')
                AND public.c4_valid_json_uuid(body->'delivery_id')
                AND public.c4_valid_json_uuid(body->'contract_id')
                AND public.c4_valid_json_uuid(body->'obligation_id')
                AND public.c4_valid_json_version(body->'round');
        WHEN 'start_attempt' THEN
            RETURN status = 200 AND disposition = 'attempt_started'
                AND public.c4_valid_json_uuid(body->'round_id')
                AND public.c4_valid_json_uuid(body->'delivery_id')
                AND public.c4_valid_json_uuid(body->'attempt_id')
                AND public.c4_valid_json_version(body->'fence');
        WHEN 'mark_write_started' THEN
            RETURN status = 200 AND disposition = 'write_started'
                AND public.c4_valid_json_uuid(body->'delivery_id')
                AND public.c4_valid_json_uuid(body->'attempt_id')
                AND public.c4_valid_json_version(body->'fence');
        WHEN 'record_evidence' THEN
            RETURN status = 200 AND disposition = 'evidence_recorded'
                AND public.c4_valid_json_uuid(body->'attempt_id')
                AND public.c4_valid_json_uuid(body->'evidence_id');
        WHEN 'acquire_return' THEN
            RETURN status = 200 AND disposition = 'return_acquired'
                AND public.c4_valid_json_uuid(body->'return_intake_id')
                AND public.c4_valid_json_version(body->'original_round')
                AND public.c4_valid_json_uuid(body->'take_work_id')
                AND public.c4_valid_json_uuid(body->'take_delivery_id');
        WHEN 'start_return_take' THEN
            RETURN status = 200 AND disposition = 'write_started'
                AND public.c4_valid_json_uuid(body->'return_intake_id')
                AND public.c4_valid_json_uuid(body->'work_id')
                AND public.c4_valid_json_uuid(body->'attempt_id')
                AND public.c4_valid_json_version(body->'fence');
        WHEN 'consume_return' THEN
            RETURN status = 200 AND disposition = 'return_consumed'
                AND public.c4_valid_json_uuid(body->'contract_id')
                AND public.c4_valid_json_version(body->'contract_version')
                AND public.c4_valid_json_uuid(body->'return_intake_id')
                AND public.c4_valid_json_uuid(body->'consumption_id')
                AND public.c4_valid_json_uuid(body->'proof_id');
        WHEN 'register_opening_attribution' THEN
            RETURN status = 200 AND disposition = 'attribution_registered'
                AND public.c4_valid_json_uuid(body->'receipt_key_id')
                AND body->>'uid' COLLATE "C" ~ '^[1-9][0-9]{0,18}$'
                AND public.c6_valid_json_amount(body->'attributed_amount');
        WHEN 'certify_cod' THEN
            RETURN status = 200 AND disposition = 'payment_funded'
                AND public.c4_valid_json_uuid(body->'contract_id')
                AND public.c4_valid_json_version(body->'contract_version')
                AND public.c4_valid_json_uuid(body->'obligation_id')
                AND public.c4_valid_json_uuid(body->'receipt_source_id')
                AND public.c6_valid_json_split(
                    body->'gross', body->'opening_included', body->'new_issue');
        WHEN 'prove_handoff' THEN
            RETURN status = 200 AND disposition = 'original_claimed'
                AND public.c4_valid_json_uuid(body->'contract_id')
                AND public.c4_valid_json_version(body->'contract_version')
                AND public.c4_valid_json_uuid(body->'obligation_id')
                AND public.c4_valid_json_uuid(body->'proof_id')
                AND public.c4_valid_json_uuid(body->'listing_id')
                AND public.c4_valid_json_version(body->'listing_version')
                AND body->>'state' = 'closed';
        WHEN 'settle_cash' THEN
            RETURN status = 200 AND disposition = 'cash_advanced'
                AND public.c4_valid_json_uuid(body->'contract_id')
                AND public.c4_valid_json_version(body->'contract_version')
                AND public.c4_valid_json_uuid(body->'obligation_id')
                AND body->>'stage' IN ('reserved', 'in_flight', 'unknown', 'spent', 'claimed',
                                       'returned', 'retained');
        WHEN 'convert_refund' THEN
            RETURN status = 200 AND disposition = 'refund_funded'
                AND public.c4_valid_json_uuid(body->'contract_id')
                AND public.c4_valid_json_version(body->'contract_version')
                AND public.c4_valid_json_uuid(body->'obligation_id');
        WHEN 'open_sanction' THEN
            RETURN status = 200 AND disposition = 'sanction_pending'
                AND public.c4_valid_json_uuid(body->'cycle_id')
                AND public.c4_valid_json_uuid(body->'sanction_id');
        ELSE
            RETURN false;
    END CASE;
END
$function$;


ALTER TABLE public.consignment_commands
    DROP CONSTRAINT c4_command_typed_target_ck;
ALTER TABLE public.consignment_commands
    ADD CONSTRAINT c4_command_typed_target_ck CHECK ((
        CASE schema_version
            WHEN 1 THEN target_kind IS NULL AND target_id IS NULL
                AND target_bot_uid IS NULL AND target_world_id IS NULL
                AND target_external_mid IS NULL AND target_season_id IS NULL
                AND target_calendar_version IS NULL
            WHEN 2 THEN target_kind IN (
                'listing', 'reservation', 'quarantine', 'mail_key',
                'final_decision', 'original_attempt', 'season',
                'dispatch_round', 'dispatch_attempt', 'return_intake', 'obligation',
                'receipt_key', 'buyer_cycle')
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
                WHEN 'public_user' THEN operation IN (
                    'issue_reservation', 'withdraw', 'purchase', 'reprice')
                    AND public.consignment_positive_uuid(actor_ref)
                WHEN 'admin' THEN operation IN (
                    'recheck_quarantine', 'release_quarantine', 'admin_delist', 'release_hold')
                    AND public.consignment_valid_text(actor_ref, 256) AND strpos(actor_ref, ':') = 0
                WHEN 'worker' THEN operation IN (
                    'expire_reservation', 'decide_envelope', 'append_quarantine_observation',
                    'start_disposition_write', 'apply_mail_proof', 'season_notice', 'season_close',
                    'authorize_dispatch', 'start_attempt', 'mark_write_started', 'record_evidence',
                    'acquire_return', 'start_return_take', 'consume_return',
                    'register_opening_attribution', 'certify_cod', 'prove_handoff', 'settle_cash',
                    'convert_refund', 'open_sanction')
                    AND public.consignment_valid_text(actor_ref, 256)
                WHEN 'system' THEN operation IN (
                    'release_quarantine', 'recheck_quarantine', 'acquire_return')
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
                    AND operation IN ('issue_reservation', 'withdraw', 'admin_delist', 'release_hold',
                                      'purchase', 'reprice'))
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
                    AND listing_id IS NULL AND operation IN ('decide_envelope', 'acquire_return'))
                OR (target_kind = 'season'
                    AND request_target = target_season_id AND listing_id IS NULL
                    AND operation IN ('season_notice', 'season_close'))
                OR (target_kind = 'dispatch_round'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation IN ('authorize_dispatch', 'start_attempt'))
                OR (target_kind = 'dispatch_attempt'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation IN ('mark_write_started', 'record_evidence'))
                OR (target_kind = 'return_intake'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation IN ('start_return_take', 'consume_return'))
                OR (target_kind = 'receipt_key'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation = 'register_opening_attribution')
                OR (target_kind = 'obligation'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation IN ('certify_cod', 'prove_handoff', 'settle_cash',
                                      'convert_refund'))
                OR (target_kind = 'buyer_cycle'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation = 'open_sanction')
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
                    'participant_ineligible', 'bot_party', 'action_not_allowed', 'self_purchase'))
                OR (result_status = 404 AND result_code IN ('consignment_disabled', 'listing_not_found'))
                OR (result_status = 409 AND result_code IN (
                    'stale_version', 'listing_not_active', 'equipment_busy', 'equipment_committed',
                    'custody_unproven', 'operations_hold', 'season_closed', 'hold_mismatch',
                    'source_mismatch', 'evidence_unproven', 'evidence_conflict',
                    'disposition_conflict', 'fence_mismatch', 'price_changed', 'funding_boundary',
                    'opposite_payment_open', 'lease_lost'))
                OR (result_status = 429 AND result_code = 'listing_limit_reached')
                OR (result_status = 503 AND result_code IN ('mail_unavailable', 'funding_unavailable'))
            )
        )
    );


-- ⑫ 挂单迁移：放开 available -> sale_pending（购买）与 sale_pending -> closed(sold)（确领原件）。
-- 整函数以 000134 文本为底，差异仅这两条边与对应的同事务事实核对。
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
                            'withdrawal_pending', 'sale_pending')
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
        IF NEW.state = 'sale_pending' THEN
            -- 购买（000136）：同事务必须已写入本挂单的 Sale 合同，且 position 已进入 committed。
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at
               OR NEW.reason_code IS NOT NULL
               OR NOT EXISTS (
                   SELECT 1 FROM public.consignment_contracts AS contract
                   JOIN public.consignment_custody_positions AS position_row
                     ON position_row.id = contract.position_id
                   WHERE contract.listing_id = NEW.id AND contract.kind = 'sale'
                     AND contract.xmin = pg_current_xact_id()::xid
                     AND position_row.state = 'committed'
                     AND position_row.xmin = pg_current_xact_id()::xid
               ) THEN
                RAISE EXCEPTION '寄售购买必须同事务写入 Sale 合同与已承诺 position，且不能改价、快照或期限'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSIF NEW.state = 'held_off_market' THEN
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
    ELSIF OLD.state = 'sale_pending' THEN
        -- 成交（000136）：只有买家确领原件（position 同事务进入 handed_over）才关闭为 sold；
        -- 未付收回、止售转移与撤回留第 7 期。
        IF NEW.state = 'closed' THEN
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at
               OR NEW.reason_code IS DISTINCT FROM 'sold'
               OR NOT EXISTS (
                   SELECT 1 FROM public.consignment_custody_positions AS position_row
                   WHERE position_row.listing_id = NEW.id AND position_row.state = 'handed_over'
                     AND position_row.xmin = pg_current_xact_id()::xid
               ) THEN
                RAISE EXCEPTION '寄售成交关闭必须同事务确领原件，且不能改价、快照或期限'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSE
            RAISE EXCEPTION '寄售已成交待履约态不能迁移到该状态'
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

    -- 第 6 期不能产生交付撤回与损失关闭；成交（sale_pending / sold）已在上面放开。
    IF NEW.state = 'closed' AND NEW.reason_code IN ('withdrawn_delivered', 'loss_confirmed') THEN
        RAISE EXCEPTION '第 6 期不能产生该挂单迁移'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_listing_phase4_transition';
    END IF;
    RETURN NEW;
END
$function$;


-- 挂单版本的同事务事件核对：整函数以 000134 文本为底，新增 purchased / repriced(v2) / 确领关闭三个分支。
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
                    WHEN 'purchased' THEN actor_kind = 'public_user'
                        AND to_state = 'sale_pending' AND reason_code IS NULL
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
            WHEN from_state = 'sale_pending' THEN
                CASE event_kind
                    WHEN 'closed' THEN actor_kind = 'worker'
                        AND to_state = 'closed' AND reason_code = 'sold'
                END
            WHEN from_state = 'withdrawal_pending' THEN
                CASE event_kind
                    WHEN 'season_closed' THEN actor_kind IN ('worker', 'system')
                        AND to_state = 'closed' AND reason_code = 'season_closed'
                END
        END
    ) IS TRUE);

-- 成功回执必须与同版本事件同事务：新增 purchased。
DROP TRIGGER consignment_command_version_event ON public.consignment_commands;
CREATE CONSTRAINT TRIGGER consignment_command_version_event
AFTER INSERT ON public.consignment_commands DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW
WHEN (NEW.result_status < 400 AND NEW.result_body->>'disposition' IN
    ('registered', 'repriced', 'closed', 'published', 'rejected', 'expired',
     'reservation_issued', 'hold_set', 'hold_released', 'purchased'))
EXECUTE FUNCTION public.consignment_check_command_event();


-- ⑬ position 状态机：以 000135 文本为底，新增购买 / 在途 / 退回 / 核对 / 交付 / 季末（awaiting_take）迁移。
-- 在途与已承诺的季末终态（committed / in_transit / reconcile -> season_ended）与 lost 留第 7 期。
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
    ELSIF OLD.state IN ('in_custody', 'committed', 'in_transit') AND NEW.state = 'reconcile' THEN
        IF NEW.current_generation IS DISTINCT FROM OLD.current_generation THEN
            RAISE EXCEPTION '核对不能替换当前处分代'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_position_transition';
        END IF;
    ELSIF OLD.state = 'reconcile' AND NEW.state = 'in_custody' THEN
        IF NEW.current_generation IS DISTINCT FROM OLD.current_generation THEN
            RAISE EXCEPTION '核对返回不能替换当前处分代'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_position_transition';
        END IF;
    ELSIF OLD.state IN ('in_custody', 'reconcile') AND NEW.state = 'season_ended' THEN
        IF NEW.current_generation IS DISTINCT FROM OLD.current_generation
           OR NEW.season_window_id IS NULL
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_season_windows AS window_row
               WHERE window_row.id = NEW.season_window_id
                 AND window_row.end_utc <= transaction_timestamp()
           ) THEN
            RAISE EXCEPTION 'season_ended 只能在季窗结束（含等号）之后且不替换处分代'
                USING ERRCODE = '23514', CONSTRAINT = 'c5_position_season_end';
        END IF;
    ELSIF OLD.state = 'awaiting_take' AND NEW.state = 'season_ended' THEN
        -- 季末关闭未起写的取得工作（R5-C3）：季窗已结束、尚无处分代、该件取得工作没有起写事实，
        -- 且同事务写入了该工作的关闭记录；已起写的保留核对，不在此终结（不变量 4）。
        IF NEW.current_generation IS NOT NULL OR NEW.season_window_id IS NULL
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_season_windows AS window_row
               WHERE window_row.id = NEW.season_window_id
                 AND window_row.end_utc <= transaction_timestamp()
           )
           OR EXISTS (
               SELECT 1 FROM public.mail_disposition_write_starts AS started
               JOIN public.mail_disposition_work AS take_work ON take_work.id = started.work_id
               JOIN public.consignment_deposit_items AS item ON item.decision_id = take_work.decision_id
               WHERE item.id = NEW.item_id
           )
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_work_closures AS closure
               JOIN public.mail_disposition_work AS take_work ON take_work.id = closure.work_id
               JOIN public.consignment_deposit_items AS item ON item.decision_id = take_work.decision_id
               WHERE item.id = NEW.item_id AND closure.position_id = NEW.id
                 AND closure.xmin = pg_current_xact_id()::xid
           ) THEN
            RAISE EXCEPTION 'awaiting_take 只能在季末且未起写、已同事务关闭取得工作时终结'
                USING ERRCODE = '23514', CONSTRAINT = 'c6_position_take_season_end';
        END IF;
    ELSIF OLD.state = 'in_custody' AND NEW.state = 'committed' THEN
        -- 购买：同事务必须写入本 position 当前处分代的 Sale 合同。
        IF NEW.current_generation IS DISTINCT FROM OLD.current_generation
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_contracts AS contract
               WHERE contract.position_id = NEW.id AND contract.generation = NEW.current_generation
                 AND contract.kind = 'sale' AND contract.xmin = pg_current_xact_id()::xid
           ) THEN
            RAISE EXCEPTION '购买必须同事务写入当前处分代的 Sale 合同'
                USING ERRCODE = '23514', CONSTRAINT = 'c6_position_commit_fact';
        END IF;
    ELSIF OLD.state = 'committed' AND NEW.state = 'in_transit' THEN
        -- 资产腿的首次起写：同事务写入该合同原件腿的派发尝试。
        IF NEW.current_generation IS DISTINCT FROM OLD.current_generation
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_dispatch_attempts AS attempt
               JOIN public.consignment_contracts AS contract ON contract.contract_id = attempt.contract_id
               WHERE contract.position_id = NEW.id AND contract.generation = NEW.current_generation
                 AND attempt.leg = 'original' AND attempt.xmin = pg_current_xact_id()::xid
           ) THEN
            RAISE EXCEPTION '在途必须同事务写入原件腿的派发尝试'
                USING ERRCODE = '23514', CONSTRAINT = 'c6_position_transit_fact';
        END IF;
    ELSIF OLD.state = 'in_transit' AND NEW.state = 'committed' THEN
        -- 原件被退回并消费：同事务写入该合同原件腿的退件消费。
        IF NEW.current_generation IS DISTINCT FROM OLD.current_generation
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_return_consumptions AS consumption
               JOIN public.consignment_contracts AS contract
                 ON contract.contract_id = consumption.contract_id
               WHERE contract.position_id = NEW.id AND contract.generation = NEW.current_generation
                 AND consumption.leg = 'original' AND consumption.xmin = pg_current_xact_id()::xid
           ) THEN
            RAISE EXCEPTION '原件退回必须同事务写入原件腿的退件消费'
                USING ERRCODE = '23514', CONSTRAINT = 'c6_position_return_fact';
        END IF;
    ELSIF OLD.state = 'reconcile' AND NEW.state IN ('committed', 'in_transit') THEN
        -- 核对结束回到合同状态：处分代不变，且当前代确有合同。
        IF NEW.current_generation IS DISTINCT FROM OLD.current_generation
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_contracts AS contract
               WHERE contract.position_id = NEW.id AND contract.generation = NEW.current_generation
           ) THEN
            RAISE EXCEPTION '核对返回已承诺状态必须保持处分代且当前代有合同'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_position_transition';
        END IF;
    ELSIF OLD.state IN ('committed', 'in_transit') AND NEW.state = 'handed_over' THEN
        -- 买家确领原件：同事务写入本 position 当前代的确领证明（不是邮件证明）。
        IF NEW.current_generation IS DISTINCT FROM OLD.current_generation
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_handoff_proofs AS handoff
               WHERE handoff.position_id = NEW.id AND handoff.generation = NEW.current_generation
                 AND handoff.xmin = pg_current_xact_id()::xid
           ) THEN
            RAISE EXCEPTION '已交付必须同事务写入当前处分代的确领证明'
                USING ERRCODE = '23514', CONSTRAINT = 'c6_position_handoff_fact';
        END IF;
    ELSIF NEW.state = 'lost' THEN
        RAISE EXCEPTION '第 6 期不能产生该 position 迁移'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_position_phase4_transition';
    ELSE
        RAISE EXCEPTION 'position 迁移不合法'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_position_transition';
    END IF;
    RETURN NEW;
END
$function$;


-- 占用释放守卫：以 000135 文本为底，只在 handed_over 分支改认 consignment_handoff_proofs。
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
        -- 季末终态（000135）：事件无邮件证明，改由“本事务写入的季末事件加 position 终态、
        -- 季窗已结束（等号即失效）”证明；lost 仍必须带同事务的邮件证明；handed_over 在 000136 改认确领证明。
        IF event_row.event_kind = 'season_ended' THEN
            IF event_row.proof_id IS NOT NULL
               OR NEW.last_fact_id <> position_row.id
               OR position_row.id IS NULL
               OR position_row.state <> event_row.to_state
               OR position_row.xmin <> pg_current_xact_id()::xid
               OR position_row.season_window_id IS NULL
               OR NOT EXISTS (
                   SELECT 1 FROM public.consignment_season_windows AS window_row
                   WHERE window_row.id = position_row.season_window_id
                     AND window_row.end_utc <= transaction_timestamp()
               ) THEN
                RAISE EXCEPTION '季末释放 position 缺少本事务的季末终态事件与已结束季窗'
                    USING ERRCODE = '23514', CONSTRAINT = 'c5_claim_season_end_fact';
            END IF;
            RETURN NULL;
        END IF;
        -- 买家确领（000136）：handed_over 事件绑定本事务写入的确领证明，占用释放的 last_fact 即该证明。
        IF event_row.event_kind = 'handed_over' THEN
            IF event_row.handoff_proof_id IS NULL
               OR event_row.proof_id IS NOT NULL
               OR NEW.last_fact_id <> event_row.handoff_proof_id
               OR position_row.id IS NULL
               OR position_row.state <> 'handed_over'
               OR position_row.xmin <> pg_current_xact_id()::xid
               OR NOT EXISTS (
                   SELECT 1 FROM public.consignment_handoff_proofs AS handoff
                   WHERE handoff.id = event_row.handoff_proof_id
                     AND handoff.position_id = old_position
                     AND handoff.xmin = pg_current_xact_id()::xid
               ) THEN
                RAISE EXCEPTION '确领释放 position 缺少本事务的 handed_over 事件与确领证明'
                    USING ERRCODE = '23514', CONSTRAINT = 'c6_claim_handoff_fact';
            END IF;
            RETURN NULL;
        END IF;
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


-- 终局处置的整封校验：整函数以 000134 文本为底，只把 acquire_return 分支由恒拒改为“恰一个 Take 工作”。
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
        -- 000136：寄售退件接管是恰一个 Take 工作且零 item 与 Return；对应 intake 由
        -- c4_reject_acquire_return_without_intake 在同事务核对。
        IF item_count <> 0 OR take_count <> 1 OR return_count <> 0 THEN
            RAISE EXCEPTION '退件接管必须恰有一个 Take 工作且零 item 与 Return'
                USING ERRCODE = '23514', CONSTRAINT = 'c4_acquire_return_intake_required';
        END IF;
        RETURN NULL;
    END IF;
    RETURN NULL;
END
$function$;


-- ⑯ acquire_return 不再恒拒：终局处置或隔离解除为 acquire_return 时，同事务必须已写入对应 K 与决定的
-- return_intake（延迟约束，COMMIT 前判定）；没有 intake 的 acquire_return 成功行仍失败关闭。
-- 已识别寄售来源且 RTS 可信时，首次决定与终局处置同为 acquire_return（不经隔离）。
ALTER TABLE public.mail_initial_decisions
    DROP CONSTRAINT c4_initial_decision_kind_ck,
    ADD CONSTRAINT c4_initial_decision_kind_ck CHECK (kind IN (
        'accepted', 'return_decided', 'archived_empty', 'quarantined', 'acquire_return'));
DROP TRIGGER c4_final_acquire_return_intake ON public.mail_final_dispositions;
DROP TRIGGER c4_release_acquire_return_intake ON public.mail_quarantine_releases;
CREATE OR REPLACE FUNCTION public.c4_reject_acquire_return_without_intake() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    decision uuid;
BEGIN
    IF NEW.kind <> 'acquire_return' THEN
        RETURN NULL;
    END IF;
    -- 终局处置表以 id 标识决定，解除表以 decision_id 引用决定。
    IF TG_TABLE_NAME = 'mail_final_dispositions' THEN
        decision := NEW.id;
    ELSE
        decision := NEW.decision_id;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM public.consignment_return_intakes AS intake
        WHERE intake.bot_uid = NEW.bot_uid AND intake.world_id = NEW.world_id
          AND intake.external_mid = NEW.external_mid AND intake.decision_id = decision
          AND intake.xmin = pg_current_xact_id()::xid
    ) THEN
        RAISE EXCEPTION 'acquire_return 成功行必须同事务写入对应的退件接管'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_acquire_return_intake_required';
    END IF;
    RETURN NULL;
END
$function$;
CREATE CONSTRAINT TRIGGER c4_final_acquire_return_intake
AFTER INSERT ON public.mail_final_dispositions
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.c4_reject_acquire_return_without_intake();
CREATE CONSTRAINT TRIGGER c4_release_acquire_return_intake
AFTER INSERT ON public.mail_quarantine_releases
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.c4_reject_acquire_return_without_intake();


-- ⑭ 事实表只追加（handoff_proofs 的触发器已在其定义处建立）；可变投影表不在此列，由逐列授权与迁移守卫保护。
DO $block$
DECLARE
    table_name text;
BEGIN
    FOREACH table_name IN ARRAY ARRAY[
        'consignment_dispatch_rounds',
        'consignment_dispatch_attempts',
        'consignment_dispatch_evidence',
        'consignment_return_intakes',
        'consignment_return_conflicts',
        'consignment_return_consumptions',
        'consignment_return_facts',
        'consignment_return_fact_verdicts',
        'consignment_refusals',
        'consignment_sanction_members',
        'consignment_work_closures',
        'consignment_cod_receipts',
        'consignment_opening_slices',
        'consignment_opening_slice_uses',
        'consignment_fee_facts',
        'consignment_principal_facts',
        'consignment_funding_returns'
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


-- ⑮ 运行账号授权入口：继承 000135 的全部授权，再对新表收紧并重授。事实表仅 SELECT、INSERT；
-- 五张可变投影表另有逐列 UPDATE（只含状态与版本列，身份列不可改）。
ALTER FUNCTION public.apply_auction_runtime_privileges()
    RENAME TO apply_auction_runtime_privileges_000135;

CREATE FUNCTION public.apply_auction_runtime_privileges()
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $function$
DECLARE
    function_owner oid;
    grantee record;
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
    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000135() FROM PUBLIC;
    FOR grantee IN
        SELECT role.rolname FROM pg_catalog.pg_roles AS role WHERE role.oid <> function_owner
    LOOP
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM %I',
            grantee.rolname
        );
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000135() FROM %I',
            grantee.rolname
        );
    END LOOP;

    PERFORM public.apply_auction_runtime_privileges_000135();

    REVOKE ALL ON TABLE
        public.consignment_handoff_proofs,
        public.consignment_dispatch_rounds,
        public.consignment_dispatch_attempts,
        public.consignment_dispatch_evidence,
        public.consignment_return_intakes,
        public.consignment_return_conflicts,
        public.consignment_return_consumptions,
        public.consignment_return_facts,
        public.consignment_return_fact_verdicts,
        public.consignment_refusals,
        public.consignment_sanction_members,
        public.consignment_work_closures,
        public.consignment_cod_receipts,
        public.consignment_opening_slices,
        public.consignment_opening_slice_uses,
        public.consignment_fee_facts,
        public.consignment_principal_facts,
        public.consignment_funding_returns,
        public.consignment_refusal_cycles,
        public.consignment_sanctions,
        public.consignment_account_work,
        public.consignment_cash_return_blocks,
        public.consignment_funded_sales
    FROM auction_runtime;
    GRANT SELECT, INSERT ON TABLE
        public.consignment_handoff_proofs,
        public.consignment_dispatch_rounds,
        public.consignment_dispatch_attempts,
        public.consignment_dispatch_evidence,
        public.consignment_return_intakes,
        public.consignment_return_conflicts,
        public.consignment_return_consumptions,
        public.consignment_return_facts,
        public.consignment_return_fact_verdicts,
        public.consignment_refusals,
        public.consignment_sanction_members,
        public.consignment_work_closures,
        public.consignment_cod_receipts,
        public.consignment_opening_slices,
        public.consignment_opening_slice_uses,
        public.consignment_fee_facts,
        public.consignment_principal_facts,
        public.consignment_funding_returns,
        public.consignment_refusal_cycles,
        public.consignment_sanctions,
        public.consignment_account_work,
        public.consignment_cash_return_blocks,
        public.consignment_funded_sales
    TO auction_runtime;
    GRANT UPDATE (state, closed_at, close_reason)
        ON TABLE public.consignment_refusal_cycles TO auction_runtime;
    GRANT UPDATE (state, version) ON TABLE public.consignment_sanctions TO auction_runtime;
    GRANT UPDATE (state, version) ON TABLE public.consignment_account_work TO auction_runtime;
    GRANT UPDATE (state, cleared_by, cleared_at)
        ON TABLE public.consignment_cash_return_blocks TO auction_runtime;
    GRANT UPDATE (disposition, version) ON TABLE public.consignment_funded_sales TO auction_runtime;

    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM PUBLIC;
    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000135() FROM PUBLIC;
    FOR grantee IN
        SELECT role.rolname FROM pg_catalog.pg_roles AS role WHERE role.oid <> function_owner
    LOOP
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM %I',
            grantee.rolname
        );
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000135() FROM %I',
            grantee.rolname
        );
    END LOOP;
END
$function$;

REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM PUBLIC, auction_runtime;
SELECT public.apply_auction_runtime_privileges();

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (136, 'breaking');
