-- 寄售第 5 期：合同与 Sale / Withdrawal 子表、结算义务来源列、position 季末终态与图完整性。
-- owner: auction-storage（寄售合同、结算义务来源、代管占用释放、维护授权）
-- 不变量 1、3、6、8、10：合同事实只追加且身份冻结；义务恰有拍卖或寄售一个来源，来源列不可改写；
-- 金额守恒 F = floor(P * fee_bps / 10000)、S = P - F 由 CHECK 兜底；合同图（合同、子项、义务、履约）
-- 在 COMMIT 前整体校验，缺一即回滚；季末终态只在季末等号及之后，占用释放仅对该终态免邮件证明。
-- 等级：breaking。拍卖义务表的 auction_id / lot_id / origin_command_id / equipment_id 放宽为可空，
-- 旧程序读到寄售义务行会解码失败；同时替换 position 与占用释放守卫并新增唯一约束，须在停机门禁下
-- 与同版程序一起切换。
--
-- 开工前核实（rg 与 000134 目录）：
--   1. deliveries 上没有按 origin_kind='settlement' 反查 settlement_obligations 与 lots 的触发器或约束，
--      现有 deliveries 触发器只绑定 cod_returned 处置与退件动作，无需放宽来源分支。
--   2. users.username 只有 btrim 与非空约束、没有长度上限；冻结身份取 128 字节（HV 角色名远小于此）。
--   3. consignment_check_command_event 只核对 result_body 的 listing_version / disposition 与同事务事件，
--      对 season_close 命令附带的 custody 事件没有限制。

LOCK TABLE
    public.settlement_obligations,
    public.settlement_obligation_fulfillments,
    public.consignment_custody_positions,
    public.consignment_custody_events
IN SHARE ROW EXCLUSIVE MODE;

-- ① 既有表上的支撑唯一约束。
ALTER TABLE public.consignment_custody_positions
    ADD CONSTRAINT c5_position_contract_identity_uq
        UNIQUE (id, listing_id, seller_user_id, equipment_id, world_id, canonical_eid),
    -- 一个挂单至多一个 position；v2 读模型按挂单取 position。
    ADD CONSTRAINT c5_position_listing_uq UNIQUE (listing_id);

-- ② 合同与两个子表。
CREATE TABLE public.consignment_contracts (
    contract_id uuid NOT NULL CONSTRAINT c5_contract_pk PRIMARY KEY,
    kind text NOT NULL,
    origin_command_id uuid NOT NULL CONSTRAINT c5_contract_command_fk
        REFERENCES public.consignment_commands(command_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED,
    listing_id uuid NOT NULL,
    position_id uuid NOT NULL,
    generation bigint NOT NULL,
    equipment_id uuid NOT NULL,
    world_id text NOT NULL,
    canonical_eid text NOT NULL,
    season_window_id uuid,
    seller_user_id uuid NOT NULL CONSTRAINT c5_contract_seller_fk
        REFERENCES public.users(id) ON DELETE RESTRICT,
    frozen_seller_identity text NOT NULL,
    created_at timestamptz NOT NULL,
    CONSTRAINT c5_contract_id_ck CHECK (public.c4_valid_uuid(contract_id)),
    CONSTRAINT c5_contract_kind_ck CHECK (kind IN ('sale', 'withdrawal')),
    CONSTRAINT c5_contract_world_ck CHECK (world_id IN ('main', 'isekai')),
    CONSTRAINT c5_contract_generation_ck CHECK (generation BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c5_contract_season_shape_ck CHECK (world_id = 'isekai' OR season_window_id IS NULL),
    CONSTRAINT c5_contract_identity_ck CHECK (public.consignment_valid_text(frozen_seller_identity, 128)),
    CONSTRAINT c5_contract_time_ck CHECK (isfinite(created_at)),
    CONSTRAINT c5_contract_origin_uq UNIQUE (origin_command_id),
    CONSTRAINT c5_contract_generation_uq UNIQUE (position_id, generation),
    CONSTRAINT c5_contract_kind_uq UNIQUE (contract_id, kind),
    CONSTRAINT c5_contract_equipment_uq UNIQUE (contract_id, equipment_id),
    CONSTRAINT c5_contract_world_uq UNIQUE (contract_id, world_id),
    CONSTRAINT c5_contract_generation_fk FOREIGN KEY (position_id, generation)
        REFERENCES public.consignment_disposition_generations (position_id, generation)
        ON DELETE RESTRICT,
    CONSTRAINT c5_contract_position_fk
        FOREIGN KEY (position_id, listing_id, seller_user_id, equipment_id, world_id, canonical_eid)
        REFERENCES public.consignment_custody_positions
            (id, listing_id, seller_user_id, equipment_id, world_id, canonical_eid)
        ON DELETE RESTRICT,
    CONSTRAINT c5_contract_asset_fk FOREIGN KEY (equipment_id, world_id, canonical_eid)
        REFERENCES public.consignment_custody_assets (equipment_id, world_id, canonical_eid)
        ON DELETE RESTRICT,
    CONSTRAINT c5_contract_season_fk FOREIGN KEY (season_window_id, position_id)
        REFERENCES public.consignment_custody_positions (season_window_id, id) ON DELETE RESTRICT
);
CREATE INDEX c5_contract_listing_idx
    ON public.consignment_contracts (listing_id, created_at, contract_id);
CREATE INDEX c5_contract_seller_idx
    ON public.consignment_contracts (seller_user_id, created_at, contract_id);

CREATE TABLE public.consignment_sales (
    contract_id uuid NOT NULL CONSTRAINT c5_sale_pk PRIMARY KEY,
    kind text NOT NULL DEFAULT 'sale',
    buyer_user_id uuid NOT NULL CONSTRAINT c5_sale_buyer_fk
        REFERENCES public.users(id) ON DELETE RESTRICT,
    frozen_buyer_identity text NOT NULL,
    listing_version bigint NOT NULL,
    price bigint NOT NULL,
    fee bigint NOT NULL,
    seller_amount bigint NOT NULL,
    pricing_version smallint NOT NULL,
    fee_bps smallint NOT NULL,
    payment_deadline_days smallint NOT NULL,
    payment_deadline_at timestamptz NOT NULL,
    reminder_limit smallint NOT NULL,
    cod_return_limit smallint NOT NULL,
    isekai_refusal_limit smallint NOT NULL,
    isekai_refusal_window_days smallint NOT NULL,
    funding_uid text NOT NULL CONSTRAINT c5_sale_funding_fk
        REFERENCES public.settlement_funding_accounts(uid) ON DELETE RESTRICT,
    funding_mode text NOT NULL,
    funding_boundary_at timestamptz NOT NULL,
    CONSTRAINT c5_sale_kind_ck CHECK (kind = 'sale'),
    CONSTRAINT c5_sale_contract_fk FOREIGN KEY (contract_id, kind)
        REFERENCES public.consignment_contracts (contract_id, kind) ON DELETE RESTRICT,
    CONSTRAINT c5_sale_identity_ck CHECK (public.consignment_valid_text(frozen_buyer_identity, 128)),
    CONSTRAINT c5_sale_version_ck CHECK (listing_version BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c5_sale_price_ck CHECK (price BETWEEN 1 AND 1000000000),
    CONSTRAINT c5_sale_pricing_ck CHECK (pricing_version = 1 AND fee_bps BETWEEN 0 AND 9999),
    -- 金额守恒：F = floor(P * fee_bps / 10000)，S = P - F，且卖家所得为正。
    CONSTRAINT c5_sale_quote_ck CHECK (
        fee = price * fee_bps / 10000 AND seller_amount = price - fee AND seller_amount > 0
    ),
    CONSTRAINT c5_sale_policy_ck CHECK (
        payment_deadline_days BETWEEN 1 AND 90 AND reminder_limit = 1
        AND cod_return_limit BETWEEN 2 AND 10 AND isekai_refusal_limit BETWEEN 2 AND 100
        AND isekai_refusal_window_days = 90
    ),
    CONSTRAINT c5_sale_funding_mode_ck CHECK (funding_mode IN ('strict_cash', 'record_only')),
    CONSTRAINT c5_sale_time_ck CHECK (
        isfinite(payment_deadline_at) AND isfinite(funding_boundary_at)
    )
);
CREATE INDEX c5_sale_buyer_idx ON public.consignment_sales (buyer_user_id, contract_id);

-- Withdrawal 没有买家、金额与费用列：“买家和金额均无”由列缺失表达，而不是靠 NULL 约定。
CREATE TABLE public.consignment_withdrawals (
    contract_id uuid NOT NULL CONSTRAINT c5_withdrawal_pk PRIMARY KEY,
    kind text NOT NULL DEFAULT 'withdrawal',
    intent_id uuid CONSTRAINT c5_withdrawal_intent_fk
        REFERENCES public.consignment_custody_controls(id) ON DELETE RESTRICT,
    predecessor_contract_id uuid CONSTRAINT c5_withdrawal_predecessor_fk
        REFERENCES public.consignment_contracts(contract_id) ON DELETE RESTRICT,
    CONSTRAINT c5_withdrawal_kind_ck CHECK (kind = 'withdrawal'),
    CONSTRAINT c5_withdrawal_contract_fk FOREIGN KEY (contract_id, kind)
        REFERENCES public.consignment_contracts (contract_id, kind) ON DELETE RESTRICT
);

-- 三张合同表只追加：事实不改写、不删除、不清空（不变量 8）。
CREATE TRIGGER c5_contracts_append_only
BEFORE UPDATE OR DELETE ON public.consignment_contracts
FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER c5_contracts_no_truncate
BEFORE TRUNCATE ON public.consignment_contracts
FOR EACH STATEMENT EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER c5_sales_append_only
BEFORE UPDATE OR DELETE ON public.consignment_sales
FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER c5_sales_no_truncate
BEFORE TRUNCATE ON public.consignment_sales
FOR EACH STATEMENT EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER c5_withdrawals_append_only
BEFORE UPDATE OR DELETE ON public.consignment_withdrawals
FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER c5_withdrawals_no_truncate
BEFORE TRUNCATE ON public.consignment_withdrawals
FOR EACH STATEMENT EXECUTE FUNCTION public.consignment_reject_fact_mutation();

-- ③ 义务来源：拍卖行四列全有、寄售行四列全空并带合同列。
ALTER TABLE public.settlement_obligations
    ALTER COLUMN auction_id DROP NOT NULL,
    ALTER COLUMN lot_id DROP NOT NULL,
    ALTER COLUMN origin_command_id DROP NOT NULL,
    ALTER COLUMN equipment_id DROP NOT NULL,
    ADD COLUMN contract_id uuid,
    ADD COLUMN contract_kind text,
    ADD COLUMN custody_equipment_id uuid;

ALTER TABLE public.settlement_obligations
    ADD CONSTRAINT c5_obligation_origin_ck CHECK (
        (auction_id IS NOT NULL AND lot_id IS NOT NULL AND origin_command_id IS NOT NULL
            AND equipment_id IS NOT NULL
            AND contract_id IS NULL AND contract_kind IS NULL AND custody_equipment_id IS NULL)
        OR
        (auction_id IS NULL AND lot_id IS NULL AND origin_command_id IS NULL
            AND equipment_id IS NULL AND submission_id IS NULL
            AND contract_id IS NOT NULL AND contract_kind IN ('sale', 'withdrawal')
            AND custody_equipment_id IS NOT NULL)
    ),
    ADD CONSTRAINT c5_obligation_kind_matrix_ck CHECK (
        contract_kind IS NULL
        OR (contract_kind = 'sale' AND obligation_kind IN (
            'winner_equipment_cod', 'winner_payment_carrier_cod', 'winner_equipment_handoff',
            'seller_payout', 'buyer_refund'))
        OR (contract_kind = 'withdrawal' AND obligation_kind = 'withdrawn_return')
    ),
    ADD CONSTRAINT c5_obligation_contract_fk FOREIGN KEY (contract_id, contract_kind)
        REFERENCES public.consignment_contracts (contract_id, kind) ON DELETE RESTRICT,
    ADD CONSTRAINT c5_obligation_equipment_fk FOREIGN KEY (contract_id, custody_equipment_id)
        REFERENCES public.consignment_contracts (contract_id, equipment_id) ON DELETE RESTRICT,
    ADD CONSTRAINT c5_obligation_world_fk FOREIGN KEY (contract_id, world_id)
        REFERENCES public.consignment_contracts (contract_id, world_id) ON DELETE RESTRICT,
    ADD CONSTRAINT c5_obligation_contract_kind_uq UNIQUE (contract_id, obligation_kind);

-- 运行账号对该表是整表 UPDATE，来源与合同列的不可变性只能靠触发器（不变量 6、8）。
CREATE FUNCTION public.c5_guard_obligation_origin() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF ROW(NEW.auction_id, NEW.lot_id, NEW.origin_command_id, NEW.equipment_id, NEW.submission_id,
           NEW.contract_id, NEW.contract_kind, NEW.custody_equipment_id)
       IS DISTINCT FROM
       ROW(OLD.auction_id, OLD.lot_id, OLD.origin_command_id, OLD.equipment_id, OLD.submission_id,
           OLD.contract_id, OLD.contract_kind, OLD.custody_equipment_id) THEN
        RAISE EXCEPTION '义务来源不可修改'
            USING ERRCODE = '23514', CONSTRAINT = 'c5_obligation_origin_immutable';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c5_obligation_origin_immutable
BEFORE UPDATE OF auction_id, lot_id, origin_command_id, equipment_id, submission_id,
                 contract_id, contract_kind, custody_equipment_id
ON public.settlement_obligations
FOR EACH ROW EXECUTE FUNCTION public.c5_guard_obligation_origin();

-- ④ position 季末终态与占用释放。
-- in_custody / reconcile 都可在季末等号及之后进入 season_ended（等号即失效，不变量 10）；
-- committed / in_transit / handed_over / lost 仍由后续期放开。
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
    ELSIF NEW.state IN ('committed', 'in_transit', 'handed_over', 'lost') THEN
        RAISE EXCEPTION '第 5 期不能产生该 position 迁移'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_position_phase4_transition';
    ELSE
        RAISE EXCEPTION 'position 迁移不合法'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_position_transition';
    END IF;
    RETURN NEW;
END
$function$;

ALTER TABLE public.consignment_custody_events
    DROP CONSTRAINT c4_custody_event_phase4_ck,
    ADD CONSTRAINT c5_custody_event_kinds_ck CHECK (
        event_kind IN ('accepted', 'in_custody', 'reconcile', 'season_ended')
    ),
    -- 季末终态事件没有邮件证明：证明由占用释放守卫按“同事务季末终态加已结束季窗”替代。
    ADD CONSTRAINT c5_custody_event_season_ended_ck CHECK (
        event_kind <> 'season_ended'
        OR (from_state IN ('in_custody', 'reconcile') AND proof_id IS NULL)
    );

-- 占用释放守卫整函数沿用 000134，只在“position → 空”分支新增季末终态的免证明路径。
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
        -- 季窗已结束（等号即失效）”证明；handed_over / lost 仍必须带同事务的邮件证明。
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

-- ⑤ 图完整性：合同、子项、义务与履约行在 COMMIT 前必须整体成形（不变量 3、6）。
-- 库内不复算 v5 义务 ID（需要 SHA-256）；ID 由应用层 validate_consignment_graph 在写前与加载时复核。
CREATE FUNCTION public.c5_check_contract_graph() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    checked_contract uuid := NEW.contract_id;
    contract_row public.consignment_contracts%ROWTYPE;
    sale_row public.consignment_sales%ROWTYPE;
    mismatch integer;
BEGIN
    SELECT * INTO contract_row
    FROM public.consignment_contracts WHERE contract_id = checked_contract;
    IF NOT FOUND THEN
        RAISE EXCEPTION '合同图缺少合同行'
            USING ERRCODE = '23514', CONSTRAINT = 'c5_contract_graph';
    END IF;
    IF contract_row.kind = 'sale' THEN
        SELECT * INTO sale_row
        FROM public.consignment_sales WHERE contract_id = checked_contract;
        IF NOT FOUND
           OR EXISTS (SELECT 1 FROM public.consignment_withdrawals
                      WHERE contract_id = checked_contract)
           OR sale_row.payment_deadline_at <= contract_row.created_at THEN
            RAISE EXCEPTION 'Sale 合同必须恰有一个 Sale 子项且付款期限晚于创建时间'
                USING ERRCODE = '23514', CONSTRAINT = 'c5_contract_graph';
        END IF;
    ELSIF NOT EXISTS (SELECT 1 FROM public.consignment_withdrawals
                      WHERE contract_id = checked_contract)
          OR EXISTS (SELECT 1 FROM public.consignment_sales
                     WHERE contract_id = checked_contract) THEN
        RAISE EXCEPTION 'Withdrawal 合同必须恰有一个 Withdrawal 子项且没有 Sale 子项'
            USING ERRCODE = '23514', CONSTRAINT = 'c5_contract_graph';
    END IF;

    -- 外键只绑定非空的季窗；合同季窗必须与 position 一致，NULL 也不例外。
    IF NOT EXISTS (
        SELECT 1 FROM public.consignment_custody_positions AS position_row
        WHERE position_row.id = contract_row.position_id
          AND position_row.season_window_id IS NOT DISTINCT FROM contract_row.season_window_id
    ) THEN
        RAISE EXCEPTION '合同季窗必须与 position 一致'
            USING ERRCODE = '23514', CONSTRAINT = 'c5_contract_graph';
    END IF;

    -- 义务集合必须恰为固定形状：Sale(main)=3、Sale(isekai)=4、Withdrawal=1。
    SELECT count(*) INTO mismatch
    FROM (
        SELECT shape.obligation_kind,
               shape.business_state,
               CASE shape.amount_basis
                   WHEN 'price' THEN sale_row.price
                   WHEN 'seller' THEN sale_row.seller_amount
                   ELSE 0
               END AS amount,
               shape.carrier_credit,
               CASE WHEN shape.buyer_leg THEN sale_row.buyer_user_id END AS recipient_user_id,
               CASE WHEN NOT shape.buyer_leg THEN contract_row.frozen_seller_identity
               END AS recipient_game_identity,
               shape.prerequisite_kind
        FROM (VALUES
            ('sale', 'main', 'winner_equipment_cod', 'ready', 'price', 0::bigint, true, NULL),
            ('sale', 'main', 'seller_payout', 'blocked', 'seller', 0::bigint, false,
                'winner_equipment_cod'),
            ('sale', 'main', 'buyer_refund', 'contingent', 'price', 0::bigint, true,
                'winner_equipment_cod'),
            ('sale', 'isekai', 'winner_payment_carrier_cod', 'ready', 'price', 1::bigint, true, NULL),
            ('sale', 'isekai', 'winner_equipment_handoff', 'blocked', 'zero', 0::bigint, true,
                'winner_payment_carrier_cod'),
            ('sale', 'isekai', 'seller_payout', 'blocked', 'seller', 0::bigint, false,
                'winner_equipment_handoff'),
            ('sale', 'isekai', 'buyer_refund', 'contingent', 'price', 0::bigint, true,
                'winner_payment_carrier_cod'),
            ('withdrawal', 'main', 'withdrawn_return', 'ready', 'zero', 0::bigint, false, NULL),
            ('withdrawal', 'isekai', 'withdrawn_return', 'ready', 'zero', 0::bigint, false, NULL)
        ) AS shape(contract_kind, world_id, obligation_kind, business_state, amount_basis,
                   carrier_credit, buyer_leg, prerequisite_kind)
        WHERE shape.contract_kind = contract_row.kind
          AND shape.world_id = contract_row.world_id
    ) AS expected
    FULL JOIN (
        SELECT obligation.obligation_kind, obligation.business_state, obligation.amount,
               obligation.carrier_credit, obligation.recipient_user_id,
               obligation.recipient_game_identity,
               prerequisite.obligation_kind AS prerequisite_kind
        FROM public.settlement_obligations AS obligation
        LEFT JOIN public.settlement_obligations AS prerequisite
          ON prerequisite.id = obligation.prerequisite_obligation_id
         AND prerequisite.contract_id = obligation.contract_id
        WHERE obligation.contract_id = checked_contract
    ) AS actual ON actual.obligation_kind = expected.obligation_kind
    WHERE expected.obligation_kind IS NULL
       OR actual.obligation_kind IS NULL
       OR (expected.business_state, expected.amount, expected.carrier_credit,
           expected.recipient_user_id, expected.recipient_game_identity,
           expected.prerequisite_kind)
          IS DISTINCT FROM
          (actual.business_state, actual.amount, actual.carrier_credit,
           actual.recipient_user_id, actual.recipient_game_identity,
           actual.prerequisite_kind);
    IF mismatch <> 0 THEN
        RAISE EXCEPTION '合同义务图与固定形状不符'
            USING ERRCODE = '23514', CONSTRAINT = 'c5_contract_graph';
    END IF;

    -- 每条义务恰有一行初始履约；拍卖同一规则：付款载体、卖款与退款在 main 发送。
    IF EXISTS (
        SELECT 1
        FROM public.settlement_obligations AS obligation
        LEFT JOIN public.settlement_obligation_fulfillments AS fulfillment
          ON fulfillment.obligation_id = obligation.id
        WHERE obligation.contract_id = checked_contract
          AND (fulfillment.obligation_id IS NULL
               OR fulfillment.execution_state <> 'pending'
               OR fulfillment.delivery_operation <> 'send'
               OR fulfillment.dispatch_world <> CASE
                    WHEN obligation.obligation_kind IN (
                        'winner_payment_carrier_cod', 'seller_payout', 'buyer_refund')
                        THEN 'main'
                    ELSE obligation.world_id END
               OR fulfillment.dormant IS DISTINCT FROM (obligation.business_state = 'contingent'))
    ) THEN
        RAISE EXCEPTION '合同义务缺少与形状一致的初始履约行'
            USING ERRCODE = '23514', CONSTRAINT = 'c5_contract_graph';
    END IF;
    RETURN NULL;
END
$function$;
CREATE CONSTRAINT TRIGGER c5_contract_graph_contract
AFTER INSERT ON public.consignment_contracts
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.c5_check_contract_graph();
CREATE CONSTRAINT TRIGGER c5_contract_graph_sale
AFTER INSERT ON public.consignment_sales
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.c5_check_contract_graph();
CREATE CONSTRAINT TRIGGER c5_contract_graph_withdrawal
AFTER INSERT ON public.consignment_withdrawals
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.c5_check_contract_graph();
CREATE CONSTRAINT TRIGGER c5_contract_graph_obligation
AFTER INSERT ON public.settlement_obligations
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW WHEN (NEW.contract_id IS NOT NULL)
EXECUTE FUNCTION public.c5_check_contract_graph();

-- ⑥ 运行账号授权入口：继承 000134 的全部授权，再收紧并重授三张合同表的 SELECT / INSERT。
ALTER FUNCTION public.apply_auction_runtime_privileges()
    RENAME TO apply_auction_runtime_privileges_000134;

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
    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000134() FROM PUBLIC;
    FOR grantee IN
        SELECT role.rolname FROM pg_catalog.pg_roles AS role WHERE role.oid <> function_owner
    LOOP
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM %I',
            grantee.rolname
        );
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000134() FROM %I',
            grantee.rolname
        );
    END LOOP;

    PERFORM public.apply_auction_runtime_privileges_000134();

    REVOKE ALL ON TABLE
        public.consignment_contracts,
        public.consignment_sales,
        public.consignment_withdrawals
    FROM auction_runtime;
    GRANT SELECT, INSERT ON TABLE
        public.consignment_contracts,
        public.consignment_sales,
        public.consignment_withdrawals
    TO auction_runtime;

    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM PUBLIC;
    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000134() FROM PUBLIC;
    FOR grantee IN
        SELECT role.rolname FROM pg_catalog.pg_roles AS role WHERE role.oid <> function_owner
    LOOP
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM %I',
            grantee.rolname
        );
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000134() FROM %I',
            grantee.rolname
        );
    END LOOP;
END
$function$;

REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM PUBLIC, auction_runtime;
SELECT public.apply_auction_runtime_privileges();

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (135, 'breaking');
