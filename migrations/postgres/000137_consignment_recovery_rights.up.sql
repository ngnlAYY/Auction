-- 寄售第 7 期：损失案件、受限权利、权利通知 outbox、付款提醒与重列决定。
-- owner: auction-storage（损失证据与案件、受限权利与会话、权利通知 outbox、提醒与重列决定、维护授权）
-- 不变量 1、3、4、5、6、7、8：命令事实各带账本外键；证据、事实、案件、结果、回执、权利、状态、挑战、尝试、
-- 会话与令牌全部只追加；一个 position 至多一次损失终局；人工案件没有任何金额或付款列；权利定位符只存摘要、
-- 挑战码只以密文存在于专用 outbox（不进 deliveries），失败尝试至多 5 次；通知结果一旦写下（含 unknown）
-- 工作行即不可再领取，未知结果永不自动重发；提醒每合同一次，重列决定每处置一次且冻结自动重列开关。
-- 等级：breaking。本文件放宽既有守卫、扩展命令账本操作与处置闭集（共享替换段），旧程序读到新行会解码失败；
-- 须在停机门禁下与同版程序一起切换。
--
-- 7C-4a 返工（本文件尚未发布，授权就地修改）：consignment_loss_proofs 补 season_id（D9 §5.3 / R6 §3.4 的“原世界 / 季”），
-- 命名 CHECK c7_loss_proof_season_ck；季的取值与形状跟 000134（consignment_season_windows、custody_positions、
-- custody_assets）一致，在 isekai 允许为空。
--
-- 7M-2 返工（本文件尚未发布，授权就地修改）：
--   - 代管事件证明（c7_custody_event_proof）补 UPDATE OF proof_id / event_kind / position_id，被引用一侧
--     （邮件证明、损失证据）补 BEFORE DELETE / 改键触发器，恢复原外键 ON DELETE RESTRICT 的效果（不变量 3、8）。
--   - position 指针换代与处分代后继同事务核对：依据 D9 §5.3 撤回 3（“唯一后继、当前指针 CAS、Withdrawal、事件及
--     回执同提”）、撤回 2（原物已取得或收回、旧买方交割已封闭）与 R6 7M-2 ④；只在 in_custody / committed 放行 g -> g+1。
--   - position committed -> in_custody 在 7C-3 补齐（见文件末尾 ⑪，授权重放之前）：新增事件种类 uncommitted、
--     封闭事实表 consignment_sale_closures 与原件在库判定 c7_sale_original_back；D9 / R5 / R6 没有给出该边的
--     前置与“Sale 已封闭”的事实表，这里按保守口径设计（统筹方 11-24 裁定推迟到 7C-3）。
--   - 7C-3 同时修正 c4_guard_position 的指针换代分支（只在 ⑪ 的整函数副本里）：命令账本行按 DepositWriter 的
--     写序最后才写入，行级守卫在指针更新时看不到它，所以把“同事务成功命令”的核对留给 COMMIT 前的
--     c7_check_generation_successor（它本来就核对），行级只核后继代行。
--
-- 7A-2（本文件尚未发布，授权就地修改；全部追加在文件末尾、授权重放之前，节号 ⑪）：
--   - 新表 consignment_return_dispositions：退回处置每个消费只一次（UNIQUE consumption_id），保存 7A-1 矩阵的决定与
--     决策时冻结的 auto_relist_enabled / config_digest（J12）；只追加，授权随文件头的包装函数。
--   - 命令账本闭集新增三个系统（worker）操作：resolve_return（目标 return_intake）、auto_relist（目标 listing）、
--     remind（新目标种类 contract），及其回执形状（disposition_recorded、relisted / held、reminder_queued /
--     reminder_voided）。
--   - 自动重列的 relisted 事件与 g+1 重列代后继校验放行 worker 的 auto_relist 命令（与手动 relist 同一规则）。
--   - 未付收回处置的归属（统筹方裁定，合同遗漏）：挂单边 sale_pending -> held_off_market(unpaid_recovered)。
--     consignment_guard_listing 在 ⑪ 末尾整函数改写（以 7M-2 文本为底，只新增该分支），前置是同事务的
--     consignment_return_dispositions 处置行（buyer_right 为 close_at_limit / terminate_unpaid、listing_effect 为
--     held_unpaid_recovered）与 7C-3 的 consignment_sale_closures(cancelled_unpaid) 封闭事实；
--     事件边新增 sale_pending 的 hold_set（worker、原因固定 unpaid_recovered），事件核对绑定 worker 的
--     resolve_return 命令，其回执可带 listing_changes（挂单版本、状态与 hold_reason）。

-- 7B-2（本文件尚未发布，授权就地修改；全部追加在文件末尾、授权重放之前，节号 ⑫）：
--   - 命令账本闭集新增 apply_sanction（worker，目标 sanction，回执 sanction_applied）与 reset_cycle（system，目标
--     buyer_cycle，回执 cycle_opened），三个账本 CHECK 与 c4_valid_v2_receipt 以 7A-2 文本为底只新增取值与分支。
--   - 处罚效果与账号工作结果新增状态 already_blacklisted（账号因别的途径已被禁：只记录、不引用拉黑命令、不追加
--     黑名单历史），c7_check_sanction_effect 整函数改写：该状态对应处罚 applied、工作 completed，且账号当前确为 blacklisted。

-- 7C-4b（本文件尚未发布，授权就地修改；追加在文件末尾、授权重放之前，节号 ⑬）：新表 consignment_loss_reconciles，
--   按 position 持久的只追加“无法查清”核对记录（灭失确认被拒为 evidence_conflict 时同事务写入）；授权随文件头的包装函数。

-- 7A-3（本文件尚未发布，授权就地修改；追加在文件末尾、授权重放之前，节号 ⑱）：作废的付款提醒以预检退役收口，
--   delivery_dispositions 的 check6（预检退役失败码闭集）与 check7（解除计数判定，含 ⑰ 的 season_closed）新增 consignment_notice_invalidated。
-- PIW S2-4（本文件尚未发布，授权就地修改；追加在文件末尾、授权重放之前，节号 ⑲）：整封首次决定的接管分支回执
--   decide_envelope / return_acquired；回执包装函数以 ⑯ 为底（改名 c7_valid_v2_receipt_e2），重建回执约束；
--   灭失证据改为 Admin 录入，重建 consignment_command_actor（record_position_loss_proof 从 worker 移到 admin）；
--   权利通知的账本回执 deliver_right_notice（worker，目标种类 notice_work），重建 typed_target / target 两个 CHECK。
-- PIWS4（本文件尚未发布，授权就地修改；追加在文件末尾、授权重放之前，节号 ⑳）：订单复核工作补租约列与领取快照，
--   守卫 c7_guard_order_recheck 换下 ⑯ 的只追加触发器；完成结果另表 consignment_order_recheck_results 只追加；授权随文件头的包装函数。
-- PIWX（本文件尚未发布，授权就地修改；追加在文件末尾、授权重放之前，节号 ㉒）：撤回件派发的 position 边
--   in_custody <-> in_transit（限当前代 Withdrawal 与同事务原件腿尝试 / 消费）；c4_guard_position 与代管事件 CHECK 以 ⑪ 为底重建。

-- 本段（7M-1）只含文件头与“独立新表”：全部新建，不改任何既有表、约束与触发器；共享替换段（7M-2）接在
-- 本段之后、授权重放之前，登记行始终是文件最后一条语句。
-- 文件头核对（rg 与 000134/000135/000136 文本，记录供 7M-2 使用）：
--   1. 授权入口以 000136 的 apply_auction_runtime_privileges 为底做改名链（改名为 _000136），不复制矩阵。
--   2. 7M-2 的整函数替换底本：c4_valid_v2_receipt、consignment_command_actor / _target / _result_code、
--      consignment_guard_listing、consignment_check_listing_event、c4_guard_position、c4_guard_claim_fact
--      均以 000136 的最终文本为底；consignment_command_receipt 与 c4_guard_generation、c4_generation_phase4_ck、
--      c4_custody_event_phase4_ck 的最终文本仍在 000134（000135 / 000136 只改写了
--      c4_custody_event_state_ck 与 c4_handoff_edge_phase4_ck）。
--   3. deliveries.origin_kind 的 CHECK 当前含 consignment_notice，但 c4_delivery_consignment_notice_payload_ck
--      把 template 固定为 consignment_season_notice、world 固定为 isekai；付款提醒（main 与 isekai 皆有）
--      若走该来源，必须由 7M-2 放宽该 payload CHECK（J7）。挑战码类通知走专用 outbox，不进 deliveries（J10），
--      无需新增 origin_kind。本段不改共享约束。
--   4. 开发期 SQL 内无 SHA-1 与 pgcrypto，回填权利 ID 所需的 UUIDv5 由本段的 c7_sha1 / c7_right_id 在库内计算。
--   5. 7E-2 在文件末尾（授权重放之前）追加 ⑯：以 7A-2、7B-2 之后的最新文本为底，账本新增 admin 操作 recheck / resolve
--      （contract 目标种类的 recheck / resolve 分支），重建 typed_target / actor / target / receipt 四个 CHECK，回执包装函数
--      c4_valid_v2_receipt（底本改名 c7_valid_v2_receipt_base，其余操作与拒绝一律委托底本），只追加的
--      consignment_order_rechecks / consignment_order_resolutions 及其授权。

LOCK TABLE
    public.consignment_contracts,
    public.consignment_sales,
    public.consignment_withdrawals
IN SHARE ROW EXCLUSIVE MODE;

-- ① 授权入口：改名 + 新包装（授权矩阵不复制，只在新包装里追加本期新表）。7M-2 追加其新表时改本函数的授权段。
ALTER FUNCTION public.apply_auction_runtime_privileges()
    RENAME TO apply_auction_runtime_privileges_000136;

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
    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000136() FROM PUBLIC;
    FOR grantee IN
        SELECT role.rolname FROM pg_catalog.pg_roles AS role WHERE role.oid <> function_owner
    LOOP
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM %I',
            grantee.rolname
        );
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000136() FROM %I',
            grantee.rolname
        );
    END LOOP;

    PERFORM public.apply_auction_runtime_privileges_000136();

    -- 本期全部表只有 SELECT、INSERT；可变列只有权利通知 outbox 的租约列（领取 / 续租 / 释放）、订单复核工作的租约列
    -- 与密文三列（只供凭据域 `keys reencrypt` 重封；守卫只放行整体换钥、其余列不变的更新）。
    REVOKE ALL ON TABLE
        public.consignment_loss_proofs,
        public.consignment_loss_facts,
        public.consignment_loss_cases,
        public.consignment_loss_case_outcomes,
        public.consignment_loss_receipts,
        public.consignment_rights,
        public.consignment_right_states,
        public.consignment_right_challenges,
        public.consignment_right_challenge_attempts,
        public.consignment_right_sessions,
        public.consignment_right_session_tokens,
        public.consignment_right_notice_outbox,
        public.consignment_right_notice_results,
        public.consignment_right_request_hits,
        public.consignment_sale_closures,
        public.consignment_payment_reminders,
        public.consignment_relist_decisions,
        public.consignment_return_dispositions,
        public.consignment_right_request_hits,
        public.consignment_sanction_effects,
        public.consignment_sanction_work_results,
        public.consignment_loss_reconciles,
        public.consignment_order_rechecks,
        public.consignment_order_resolutions,
        public.consignment_order_recheck_results
    FROM auction_runtime;
    GRANT SELECT, INSERT ON TABLE
        public.consignment_loss_proofs,
        public.consignment_loss_facts,
        public.consignment_loss_cases,
        public.consignment_loss_case_outcomes,
        public.consignment_loss_receipts,
        public.consignment_rights,
        public.consignment_right_states,
        public.consignment_right_challenges,
        public.consignment_right_challenge_attempts,
        public.consignment_right_sessions,
        public.consignment_right_session_tokens,
        public.consignment_right_notice_outbox,
        public.consignment_right_notice_results,
        public.consignment_right_request_hits,
        public.consignment_sale_closures,
        public.consignment_payment_reminders,
        public.consignment_relist_decisions,
        public.consignment_return_dispositions,
        public.consignment_right_request_hits,
        public.consignment_sanction_effects,
        public.consignment_sanction_work_results,
        public.consignment_loss_reconciles,
        public.consignment_order_rechecks,
        public.consignment_order_resolutions,
        public.consignment_order_recheck_results
    TO auction_runtime;
    GRANT UPDATE (claim_token, claim_fence, lease_expires_at,
                  sealed_payload, nonce, encryption_key_id)
        ON TABLE public.consignment_right_notice_outbox TO auction_runtime;
    -- 订单复核工作（⑳）：只有租约列与领取快照可更新，守卫 c7_guard_order_recheck 只放行新领取。
    GRANT UPDATE (claim_token, claim_fence, lease_expires_at,
                  pending_dispatch_attempts, pending_take_works)
        ON TABLE public.consignment_order_rechecks TO auction_runtime;

    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM PUBLIC;
    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000136() FROM PUBLIC;
    FOR grantee IN
        SELECT role.rolname FROM pg_catalog.pg_roles AS role WHERE role.oid <> function_owner
    LOOP
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM %I',
            grantee.rolname
        );
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000136() FROM %I',
            grantee.rolname
        );
    END LOOP;
END
$function$;

-- ② 权利 ID 的确定性派生（R5-C6：right_id = UUIDv5(NAMESPACE_URL, 域前缀 ‖ contract_id 原 16 字节 ‖ role)）。
-- 库内没有 SHA-1 与 pgcrypto：c7_sha1 按 FIPS 180-4 逐块实现，字节向量由 PG 测试与 Python uuid.uuid5 对照。
-- 7D-6 登记新权利必须用同一函数，保证回填行与后续登记行同键。
CREATE FUNCTION public.c7_sha1(message bytea) RETURNS bytea
LANGUAGE plpgsql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog
AS $function$
DECLARE
    mask constant bigint := 4294967295;
    padded bytea;
    bit_length bigint;
    block_start integer;
    round_index integer;
    shift integer;
    schedule bigint[];
    mixed bigint;
    h0 bigint := 1732584193;
    h1 bigint := 4023233417;
    h2 bigint := 2562383102;
    h3 bigint := 271733878;
    h4 bigint := 3285377520;
    a bigint;
    b bigint;
    c bigint;
    d bigint;
    e bigint;
    f bigint;
    k bigint;
    temp bigint;
BEGIN
    bit_length := octet_length(message)::bigint * 8;
    padded := message || '\x80'::bytea;
    WHILE octet_length(padded) % 64 <> 56 LOOP
        padded := padded || '\x00'::bytea;
    END LOOP;
    FOR shift IN REVERSE 7..0 LOOP
        padded := padded || set_byte('\x00'::bytea, 0, ((bit_length >> (shift * 8)) & 255)::integer);
    END LOOP;

    FOR block_start IN 0..(octet_length(padded) / 64 - 1) LOOP
        schedule := ARRAY[]::bigint[];
        FOR round_index IN 0..15 LOOP
            schedule[round_index + 1] :=
                (get_byte(padded, block_start * 64 + round_index * 4)::bigint << 24)
                | (get_byte(padded, block_start * 64 + round_index * 4 + 1)::bigint << 16)
                | (get_byte(padded, block_start * 64 + round_index * 4 + 2)::bigint << 8)
                | get_byte(padded, block_start * 64 + round_index * 4 + 3)::bigint;
        END LOOP;
        FOR round_index IN 16..79 LOOP
            mixed := schedule[round_index - 2] # schedule[round_index - 7]
                # schedule[round_index - 13] # schedule[round_index - 15];
            schedule[round_index + 1] := ((mixed << 1) & mask) | (mixed >> 31);
        END LOOP;

        a := h0; b := h1; c := h2; d := h3; e := h4;
        FOR round_index IN 0..79 LOOP
            IF round_index < 20 THEN
                f := (b & c) | ((mask # b) & d); k := 1518500249;
            ELSIF round_index < 40 THEN
                f := b # c # d; k := 1859775393;
            ELSIF round_index < 60 THEN
                f := (b & c) | (b & d) | (c & d); k := 2400959708;
            ELSE
                f := b # c # d; k := 3395469782;
            END IF;
            temp := (((a << 5) & mask) | (a >> 27)) + f + e + k + schedule[round_index + 1];
            temp := temp & mask;
            e := d;
            d := c;
            c := ((b << 30) & mask) | (b >> 2);
            b := a;
            a := temp;
        END LOOP;
        h0 := (h0 + a) & mask;
        h1 := (h1 + b) & mask;
        h2 := (h2 + c) & mask;
        h3 := (h3 + d) & mask;
        h4 := (h4 + e) & mask;
    END LOOP;

    RETURN decode(
        lpad(to_hex(h0), 8, '0') || lpad(to_hex(h1), 8, '0') || lpad(to_hex(h2), 8, '0')
        || lpad(to_hex(h3), 8, '0') || lpad(to_hex(h4), 8, '0'),
        'hex'
    );
END
$function$;

CREATE FUNCTION public.c7_right_id(contract_id uuid, role text) RETURNS uuid
LANGUAGE plpgsql IMMUTABLE STRICT PARALLEL SAFE SET search_path = pg_catalog, public
AS $function$
DECLARE
    digest bytea;
BEGIN
    IF role NOT IN ('buyer', 'seller') THEN
        RAISE EXCEPTION '权利角色只能是 buyer 或 seller'
            USING ERRCODE = '22023';
    END IF;
    digest := public.c7_sha1(
        uuid_send('6ba7b811-9dad-11d1-80b4-00c04fd430c8'::uuid)
        || convert_to('auction.consignment.right.v1', 'UTF8') || '\x00'::bytea
        || uuid_send(contract_id) || convert_to(role, 'UTF8')
    );
    digest := set_byte(digest, 6, (get_byte(digest, 6) & 15) | 80);
    digest := set_byte(digest, 8, (get_byte(digest, 8) & 63) | 128);
    RETURN encode(substring(digest FROM 1 FOR 16), 'hex')::uuid;
END
$function$;

-- ③ 损失证据、事实、案件、结果与卖家回执（7C-4a）。人工案件没有任何金额、付款或发行列：能力缺失而非约定。
CREATE TABLE public.consignment_loss_proofs (
    proof_id uuid NOT NULL CONSTRAINT c7_loss_proof_pk PRIMARY KEY,
    position_id uuid NOT NULL,
    world_id text NOT NULL,
    canonical_eid text NOT NULL,
    season_id text,
    last_known_ref text NOT NULL,
    movement_digest bytea NOT NULL,
    exclusion_digest bytea NOT NULL,
    source_kind text NOT NULL,
    observation_ref text NOT NULL,
    digest bytea NOT NULL,
    origin_command_id uuid NOT NULL CONSTRAINT c7_loss_proof_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    recorded_at timestamptz NOT NULL,
    CONSTRAINT c7_loss_proof_position_fk FOREIGN KEY (position_id, world_id, canonical_eid)
        REFERENCES public.consignment_custody_positions (id, world_id, canonical_eid)
        ON DELETE RESTRICT,
    CONSTRAINT c7_loss_proof_id_ck CHECK (public.c4_valid_uuid(proof_id)),
    CONSTRAINT c7_loss_proof_text_ck CHECK (
        public.consignment_valid_text(last_known_ref, 256)
        AND public.consignment_valid_text(source_kind, 64)
        AND public.consignment_valid_text(observation_ref, 128)
    ),
    -- 季语义与 000134 一致（原世界 / 季，D9 §5.3）：main 没有季；isekai 的季来自 position 绑定的季窗，
    -- 与 consignment_custody_assets.season_id 同形——position 的 season_window_id 在 isekai 可空，故季可空。
    CONSTRAINT c7_loss_proof_season_ck CHECK (
        (world_id = 'main' AND season_id IS NULL)
        OR (world_id = 'isekai'
            AND (season_id IS NULL OR public.consignment_valid_text(season_id, 128)))
    ),
    CONSTRAINT c7_loss_proof_digest_ck CHECK (
        public.c4_valid_digest(digest)
        AND public.c4_valid_digest(movement_digest)
        AND public.c4_valid_digest(exclusion_digest)
    ),
    CONSTRAINT c7_loss_proof_time_ck CHECK (isfinite(recorded_at)),
    CONSTRAINT c7_loss_proof_source_uq UNIQUE (position_id, source_kind, observation_ref),
    CONSTRAINT c7_loss_proof_position_uq UNIQUE (proof_id, position_id)
);

CREATE TABLE public.consignment_loss_facts (
    loss_fact_id uuid NOT NULL CONSTRAINT c7_loss_fact_pk PRIMARY KEY,
    position_id uuid NOT NULL,
    proof_id uuid NOT NULL,
    listing_id uuid NOT NULL,
    seller_user_id uuid NOT NULL,
    equipment_id uuid NOT NULL,
    world_id text NOT NULL,
    canonical_eid text NOT NULL,
    generation bigint NOT NULL,
    origin_command_id uuid NOT NULL CONSTRAINT c7_loss_fact_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    occurred_at timestamptz NOT NULL,
    CONSTRAINT c7_loss_fact_position_fk
        FOREIGN KEY (position_id, listing_id, seller_user_id, equipment_id, world_id, canonical_eid)
        REFERENCES public.consignment_custody_positions
            (id, listing_id, seller_user_id, equipment_id, world_id, canonical_eid)
        ON DELETE RESTRICT,
    CONSTRAINT c7_loss_fact_proof_fk FOREIGN KEY (proof_id, position_id)
        REFERENCES public.consignment_loss_proofs (proof_id, position_id) ON DELETE RESTRICT,
    -- 指针不变（D9 §5.3）：事实只记录确认时的当前代，不新增代。
    CONSTRAINT c7_loss_fact_generation_fk FOREIGN KEY (position_id, generation)
        REFERENCES public.consignment_disposition_generations (position_id, generation)
        ON DELETE RESTRICT,
    CONSTRAINT c7_loss_fact_id_ck CHECK (public.c4_valid_uuid(loss_fact_id)),
    CONSTRAINT c7_loss_fact_generation_ck CHECK (generation BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c7_loss_fact_time_ck CHECK (isfinite(occurred_at)),
    -- 一个 position 至多一次损失终局；一份证据至多支撑一个事实。
    CONSTRAINT c7_loss_fact_position_uq UNIQUE (position_id),
    CONSTRAINT c7_loss_fact_proof_uq UNIQUE (proof_id),
    CONSTRAINT c7_loss_fact_command_uq UNIQUE (origin_command_id),
    CONSTRAINT c7_loss_fact_key_uq UNIQUE (loss_fact_id, position_id, seller_user_id)
);

CREATE TABLE public.consignment_loss_cases (
    case_id uuid NOT NULL CONSTRAINT c7_loss_case_pk PRIMARY KEY,
    loss_fact_id uuid NOT NULL,
    position_id uuid NOT NULL,
    seller_user_id uuid NOT NULL,
    origin_command_id uuid NOT NULL CONSTRAINT c7_loss_case_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    created_at timestamptz NOT NULL,
    CONSTRAINT c7_loss_case_fact_fk FOREIGN KEY (loss_fact_id, position_id, seller_user_id)
        REFERENCES public.consignment_loss_facts (loss_fact_id, position_id, seller_user_id)
        ON DELETE RESTRICT,
    CONSTRAINT c7_loss_case_id_ck CHECK (public.c4_valid_uuid(case_id)),
    CONSTRAINT c7_loss_case_time_ck CHECK (isfinite(created_at)),
    CONSTRAINT c7_loss_case_key_uq UNIQUE (position_id, loss_fact_id, seller_user_id),
    CONSTRAINT c7_loss_case_seller_uq UNIQUE (case_id, seller_user_id)
);

CREATE TABLE public.consignment_loss_case_outcomes (
    outcome_id uuid NOT NULL CONSTRAINT c7_loss_outcome_pk PRIMARY KEY,
    case_id uuid NOT NULL CONSTRAINT c7_loss_outcome_case_fk
        REFERENCES public.consignment_loss_cases(case_id) ON DELETE RESTRICT,
    case_version bigint NOT NULL,
    outcome text NOT NULL,
    basis_id uuid,
    private_note text,
    command_id uuid NOT NULL CONSTRAINT c7_loss_outcome_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    occurred_at timestamptz NOT NULL,
    CONSTRAINT c7_loss_outcome_id_ck CHECK (public.c4_valid_uuid(outcome_id)),
    CONSTRAINT c7_loss_outcome_kind_ck CHECK (outcome IN ('opened', 'reviewing', 'closed')),
    -- 版本 1 恰是开案；其后每条人工结果版本 +1。
    CONSTRAINT c7_loss_outcome_version_ck CHECK (
        case_version BETWEEN 1 AND 9007199254740991
        AND ((outcome = 'opened') = (case_version = 1)) IS TRUE
    ),
    CONSTRAINT c7_loss_outcome_basis_ck CHECK (basis_id IS NULL OR public.c4_valid_uuid(basis_id)),
    CONSTRAINT c7_loss_outcome_note_ck CHECK (
        private_note IS NULL OR public.consignment_valid_text(private_note, 2000)
    ),
    CONSTRAINT c7_loss_outcome_time_ck CHECK (isfinite(occurred_at)),
    CONSTRAINT c7_loss_outcome_version_uq UNIQUE (case_id, case_version),
    CONSTRAINT c7_loss_outcome_command_uq UNIQUE (command_id),
    CONSTRAINT c7_loss_outcome_case_key_uq UNIQUE (outcome_id, case_id)
);

CREATE TABLE public.consignment_loss_receipts (
    receipt_id uuid NOT NULL CONSTRAINT c7_loss_receipt_pk PRIMARY KEY,
    case_id uuid NOT NULL,
    outcome_id uuid NOT NULL,
    seller_user_id uuid NOT NULL,
    recorded_at timestamptz NOT NULL,
    CONSTRAINT c7_loss_receipt_case_fk FOREIGN KEY (case_id, seller_user_id)
        REFERENCES public.consignment_loss_cases (case_id, seller_user_id) ON DELETE RESTRICT,
    CONSTRAINT c7_loss_receipt_outcome_fk FOREIGN KEY (outcome_id, case_id)
        REFERENCES public.consignment_loss_case_outcomes (outcome_id, case_id) ON DELETE RESTRICT,
    CONSTRAINT c7_loss_receipt_id_ck CHECK (public.c4_valid_uuid(receipt_id)),
    CONSTRAINT c7_loss_receipt_time_ck CHECK (isfinite(recorded_at)),
    CONSTRAINT c7_loss_receipt_outcome_uq UNIQUE (outcome_id)
);

-- ④ 受限权利（7D-2）。权利身份不可变；版本、状态与动作范围另表追加。
CREATE TABLE public.consignment_rights (
    right_id uuid NOT NULL CONSTRAINT c7_right_pk PRIMARY KEY,
    locator_digest bytea NOT NULL,
    beneficiary_user_id uuid NOT NULL CONSTRAINT c7_right_user_fk
        REFERENCES public.users(id) ON DELETE RESTRICT,
    frozen_identity text NOT NULL,
    world_id text NOT NULL,
    kind text NOT NULL,
    contract_id uuid,
    position_id uuid CONSTRAINT c7_right_position_fk
        REFERENCES public.consignment_custody_positions(id) ON DELETE RESTRICT,
    created_command_id uuid NOT NULL CONSTRAINT c7_right_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    created_at timestamptz NOT NULL,
    CONSTRAINT c7_right_contract_fk FOREIGN KEY (contract_id, world_id)
        REFERENCES public.consignment_contracts (contract_id, world_id) ON DELETE RESTRICT,
    CONSTRAINT c7_right_id_ck CHECK (public.c4_valid_uuid(right_id)),
    CONSTRAINT c7_right_locator_ck CHECK (public.c4_valid_digest(locator_digest)),
    CONSTRAINT c7_right_identity_ck CHECK (public.consignment_valid_text(frozen_identity, 128)),
    CONSTRAINT c7_right_world_ck CHECK (world_id IN ('main', 'isekai')),
    CONSTRAINT c7_right_kind_ck CHECK (kind IN (
        'continue_order', 'claim_equipment', 'claim_payment', 'withdraw_held', 'loss_case'
    )),
    CONSTRAINT c7_right_target_ck CHECK (num_nonnulls(contract_id, position_id) >= 1),
    CONSTRAINT c7_right_time_ck CHECK (isfinite(created_at)),
    -- 定位符摘要全局唯一：定位只靠随机量，同摘要不能指向两个权利。
    CONSTRAINT c7_right_locator_uq UNIQUE (locator_digest),
    CONSTRAINT c7_right_identity_uq
        UNIQUE NULLS NOT DISTINCT (beneficiary_user_id, kind, contract_id, position_id)
);
CREATE INDEX c7_right_beneficiary_idx
    ON public.consignment_rights (beneficiary_user_id, created_at, right_id);

CREATE TABLE public.consignment_right_states (
    right_id uuid NOT NULL CONSTRAINT c7_right_state_right_fk
        REFERENCES public.consignment_rights(right_id) ON DELETE RESTRICT,
    version bigint NOT NULL,
    state text NOT NULL,
    allowed_actions text[] NOT NULL,
    occurred_at timestamptz NOT NULL,
    CONSTRAINT c7_right_state_pk PRIMARY KEY (right_id, version),
    CONSTRAINT c7_right_state_version_ck CHECK (version BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c7_right_state_ck CHECK (state IN ('active', 'fulfilled', 'expired', 'revoked')),
    -- 动作范围是一维集合（可为空：只读权利），元素限于四个领取 / 续办动作。
    CONSTRAINT c7_right_state_actions_ck CHECK (
        (cardinality(allowed_actions) = 0 OR array_ndims(allowed_actions) = 1)
        AND allowed_actions <@ ARRAY['continue_order', 'claim_equipment', 'claim_payment', 'withdraw_held']::text[]
    ),
    CONSTRAINT c7_right_state_time_ck CHECK (isfinite(occurred_at))
);

CREATE TABLE public.consignment_right_challenges (
    challenge_id uuid NOT NULL CONSTRAINT c7_challenge_pk PRIMARY KEY,
    right_id uuid,
    is_virtual boolean NOT NULL,
    right_version bigint,
    browser_binding_digest bytea NOT NULL,
    code_mac bytea,
    nonce_digest bytea,
    identity_digest bytea,
    created_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    CONSTRAINT c7_challenge_right_fk FOREIGN KEY (right_id, right_version)
        REFERENCES public.consignment_right_states (right_id, version) ON DELETE RESTRICT,
    CONSTRAINT c7_challenge_id_ck CHECK (public.c4_valid_uuid(challenge_id)),
    CONSTRAINT c7_challenge_binding_ck CHECK (public.c4_valid_digest(browser_binding_digest)),
    -- 虚拟目标与真实目标同形可查：虚拟行没有权利、版本与任何摘要列。
    CONSTRAINT c7_challenge_shape_ck CHECK ((
        CASE
            WHEN is_virtual THEN
                right_id IS NULL AND right_version IS NULL AND code_mac IS NULL
                AND nonce_digest IS NULL AND identity_digest IS NULL
            ELSE
                right_id IS NOT NULL AND right_version IS NOT NULL
                AND public.c4_valid_digest(code_mac)
                AND public.c4_valid_digest(nonce_digest)
                AND public.c4_valid_digest(identity_digest)
        END
    ) IS TRUE),
    CONSTRAINT c7_challenge_time_ck CHECK (isfinite(created_at) AND isfinite(expires_at)),
    CONSTRAINT c7_challenge_ttl_ck CHECK (expires_at = created_at + interval '10 minutes'),
    CONSTRAINT c7_challenge_right_uq UNIQUE (challenge_id, right_id)
);
CREATE INDEX c7_challenge_right_idx
    ON public.consignment_right_challenges (right_id, created_at) WHERE right_id IS NOT NULL;

CREATE TABLE public.consignment_right_challenge_attempts (
    challenge_id uuid NOT NULL CONSTRAINT c7_attempt_challenge_fk
        REFERENCES public.consignment_right_challenges(challenge_id) ON DELETE RESTRICT,
    attempt smallint NOT NULL,
    outcome text NOT NULL,
    occurred_at timestamptz NOT NULL,
    CONSTRAINT c7_attempt_pk PRIMARY KEY (challenge_id, attempt),
    -- 失败至多 5 次：第 6 次即使码对也没有可写的尝试号。
    CONSTRAINT c7_attempt_max_ck CHECK (attempt BETWEEN 1 AND 5),
    CONSTRAINT c7_attempt_outcome_ck CHECK (outcome IN ('failed', 'consumed')),
    CONSTRAINT c7_attempt_time_ck CHECK (isfinite(occurred_at))
);
-- 一个挑战至多被消费一次。
CREATE UNIQUE INDEX c7_attempt_consumed_uq
    ON public.consignment_right_challenge_attempts (challenge_id) WHERE outcome = 'consumed';

CREATE TABLE public.consignment_right_sessions (
    session_id uuid NOT NULL CONSTRAINT c7_session_pk PRIMARY KEY,
    challenge_id uuid NOT NULL,
    right_id uuid NOT NULL,
    action_scope text[] NOT NULL,
    browser_binding_digest bytea NOT NULL,
    issued_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    CONSTRAINT c7_session_challenge_fk FOREIGN KEY (challenge_id, right_id)
        REFERENCES public.consignment_right_challenges (challenge_id, right_id) ON DELETE RESTRICT,
    CONSTRAINT c7_session_id_ck CHECK (public.c4_valid_uuid(session_id)),
    CONSTRAINT c7_session_scope_ck CHECK (
        (cardinality(action_scope) = 0 OR array_ndims(action_scope) = 1)
        AND action_scope <@ ARRAY['continue_order', 'claim_equipment', 'claim_payment', 'withdraw_held']::text[]
    ),
    CONSTRAINT c7_session_binding_ck CHECK (public.c4_valid_digest(browser_binding_digest)),
    CONSTRAINT c7_session_time_ck CHECK (isfinite(issued_at) AND isfinite(expires_at)),
    -- 原截止：轮换令牌不延寿，截止不晚于签发后 10 分钟。
    CONSTRAINT c7_session_ttl_ck CHECK (
        expires_at > issued_at AND expires_at <= issued_at + interval '10 minutes'
    ),
    -- 一次挑战至多换一个会话。
    CONSTRAINT c7_session_challenge_uq UNIQUE (challenge_id)
);

CREATE TABLE public.consignment_right_session_tokens (
    session_id uuid NOT NULL CONSTRAINT c7_session_token_session_fk
        REFERENCES public.consignment_right_sessions(session_id) ON DELETE RESTRICT,
    generation integer NOT NULL,
    token_digest bytea NOT NULL,
    issued_at timestamptz NOT NULL,
    CONSTRAINT c7_session_token_pk PRIMARY KEY (session_id, generation),
    CONSTRAINT c7_session_token_generation_ck CHECK (generation BETWEEN 1 AND 1000),
    CONSTRAINT c7_session_token_digest_ck CHECK (public.c4_valid_digest(token_digest)),
    CONSTRAINT c7_session_token_time_ck CHECK (isfinite(issued_at)),
    CONSTRAINT c7_session_token_uq UNIQUE (token_digest)
);

-- ⑤ 专用密文 outbox（D9 §5.3 / J10）：挑战码只以密文存在，不进 deliveries；每个通知一行，结果另表追加。
CREATE TABLE public.consignment_right_notice_outbox (
    work_id uuid NOT NULL CONSTRAINT c7_notice_pk PRIMARY KEY,
    notice_kind text NOT NULL,
    challenge_id uuid CONSTRAINT c7_notice_challenge_fk
        REFERENCES public.consignment_right_challenges(challenge_id) ON DELETE RESTRICT,
    right_id uuid CONSTRAINT c7_notice_right_fk
        REFERENCES public.consignment_rights(right_id) ON DELETE RESTRICT,
    loss_case_id uuid CONSTRAINT c7_notice_case_fk
        REFERENCES public.consignment_loss_cases(case_id) ON DELETE RESTRICT,
    frozen_identity text NOT NULL,
    world_id text NOT NULL,
    sealed_payload bytea,
    nonce bytea,
    encryption_key_id text,
    claim_token uuid,
    claim_fence bigint NOT NULL DEFAULT 0,
    lease_expires_at timestamptz,
    created_at timestamptz NOT NULL,
    CONSTRAINT c7_notice_id_ck CHECK (public.c4_valid_uuid(work_id)),
    CONSTRAINT c7_notice_kind_ck CHECK (
        notice_kind IN ('locator_guide', 'challenge', 'loss_receipt', 'right_notice')
    ),
    CONSTRAINT c7_notice_target_ck CHECK (num_nonnulls(challenge_id, right_id, loss_case_id) >= 1),
    CONSTRAINT c7_notice_identity_ck CHECK (
        public.consignment_valid_text(frozen_identity, 128) AND world_id IN ('main', 'isekai')
    ),
    -- 含秘密的通知（指引、挑战码）必有完整密文三元组，无秘密的通知三列全空。
    CONSTRAINT c7_notice_secret_ck CHECK ((
        CASE
            WHEN notice_kind IN ('locator_guide', 'challenge') THEN
                octet_length(sealed_payload) > 0 AND octet_length(nonce) = 12
                AND public.consignment_valid_text(encryption_key_id, 128)
            ELSE
                sealed_payload IS NULL AND nonce IS NULL AND encryption_key_id IS NULL
        END
    ) IS TRUE),
    CONSTRAINT c7_notice_lease_ck CHECK (
        claim_fence BETWEEN 0 AND 9007199254740991
        AND ((claim_token IS NULL) = (lease_expires_at IS NULL)) IS TRUE
        AND (claim_token IS NULL OR claim_fence >= 1) IS TRUE
        AND (lease_expires_at IS NULL OR isfinite(lease_expires_at)) IS TRUE
    ),
    CONSTRAINT c7_notice_time_ck CHECK (isfinite(created_at)),
    CONSTRAINT c7_notice_key_uq
        UNIQUE NULLS NOT DISTINCT (notice_kind, challenge_id, right_id, loss_case_id)
);

CREATE TABLE public.consignment_right_notice_results (
    work_id uuid NOT NULL CONSTRAINT c7_notice_result_pk PRIMARY KEY
        CONSTRAINT c7_notice_result_work_fk
        REFERENCES public.consignment_right_notice_outbox(work_id) ON DELETE RESTRICT,
    result text NOT NULL,
    occurred_at timestamptz NOT NULL,
    CONSTRAINT c7_notice_result_ck CHECK (result IN ('sent', 'unknown', 'suppressed')),
    CONSTRAINT c7_notice_result_time_ck CHECK (isfinite(occurred_at))
);

-- 找回预算（7D-2）：每个受理的浏览器 / IP 请求一行，按命令追加；窗口计数只读本表。
CREATE TABLE public.consignment_right_request_hits (
    command_id uuid NOT NULL CONSTRAINT c7_hit_pk PRIMARY KEY
        CONSTRAINT c7_hit_command_fk REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    browser_binding_digest bytea NOT NULL
        CONSTRAINT c7_hit_browser_ck CHECK (public.c4_valid_digest(browser_binding_digest)),
    client_ip_digest bytea NOT NULL
        CONSTRAINT c7_hit_ip_ck CHECK (public.c4_valid_digest(client_ip_digest)),
    occurred_at timestamptz NOT NULL CONSTRAINT c7_hit_time_ck CHECK (isfinite(occurred_at))
);
CREATE INDEX c7_hit_browser_idx
    ON public.consignment_right_request_hits (browser_binding_digest, occurred_at);
CREATE INDEX c7_hit_ip_idx
    ON public.consignment_right_request_hits (client_ip_digest, occurred_at);

-- outbox 只有租约列可变：身份与密文列不变；围栏单调且每次至多 +1；已有结果（含 unknown）的工作不可再领取，
-- 未知结果因此永不自动重发（不变量 4、5）。
CREATE FUNCTION public.c7_guard_notice_outbox() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF ROW(NEW.work_id, NEW.notice_kind, NEW.challenge_id, NEW.right_id, NEW.loss_case_id,
           NEW.frozen_identity, NEW.world_id, NEW.sealed_payload, NEW.nonce,
           NEW.encryption_key_id, NEW.created_at)
       IS DISTINCT FROM
       ROW(OLD.work_id, OLD.notice_kind, OLD.challenge_id, OLD.right_id, OLD.loss_case_id,
           OLD.frozen_identity, OLD.world_id, OLD.sealed_payload, OLD.nonce,
           OLD.encryption_key_id, OLD.created_at) THEN
        RAISE EXCEPTION '权利通知 outbox 只允许更新租约列'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_notice_immutable';
    END IF;
    IF NEW.claim_fence < OLD.claim_fence OR NEW.claim_fence > OLD.claim_fence + 1 THEN
        RAISE EXCEPTION '权利通知租约围栏必须单调且每次至多加一'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_notice_fence';
    END IF;
    IF EXISTS (
        SELECT 1 FROM public.consignment_right_notice_results AS result
        WHERE result.work_id = OLD.work_id
    ) THEN
        RAISE EXCEPTION '已有结果的权利通知不得再次领取'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_notice_finished';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c7_notice_outbox_guard
BEFORE UPDATE ON public.consignment_right_notice_outbox
FOR EACH ROW EXECUTE FUNCTION public.c7_guard_notice_outbox();
CREATE TRIGGER c7_notice_outbox_no_delete
BEFORE DELETE ON public.consignment_right_notice_outbox
FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER c7_notice_outbox_no_truncate
BEFORE TRUNCATE ON public.consignment_right_notice_outbox
FOR EACH STATEMENT EXECUTE FUNCTION public.consignment_reject_fact_mutation();

-- ⑥ 提醒与重列决定（7A-2）。
CREATE TABLE public.consignment_payment_reminders (
    contract_id uuid NOT NULL CONSTRAINT c7_reminder_pk PRIMARY KEY
        CONSTRAINT c7_reminder_sale_fk
        REFERENCES public.consignment_sales(contract_id) ON DELETE RESTRICT,
    decision text NOT NULL,
    due_at timestamptz NOT NULL,
    decided_at timestamptz NOT NULL,
    delivery_id uuid CONSTRAINT c7_reminder_delivery_fk
        REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    command_id uuid NOT NULL CONSTRAINT c7_reminder_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT c7_reminder_decision_ck CHECK (
        decision IN ('queued', 'void_paid', 'void_cancelled', 'void_round')
    ),
    -- 只有真正排队的提醒才有投递。
    CONSTRAINT c7_reminder_delivery_ck CHECK (((decision = 'queued') = (delivery_id IS NOT NULL)) IS TRUE),
    CONSTRAINT c7_reminder_time_ck CHECK (isfinite(due_at) AND isfinite(decided_at)),
    CONSTRAINT c7_reminder_delivery_uq UNIQUE (delivery_id)
);

CREATE TABLE public.consignment_relist_decisions (
    decision_id uuid NOT NULL CONSTRAINT c7_relist_pk PRIMARY KEY,
    source_consumption_id uuid NOT NULL CONSTRAINT c7_relist_source_fk
        REFERENCES public.consignment_return_consumptions(id) ON DELETE RESTRICT,
    listing_id uuid NOT NULL CONSTRAINT c7_relist_listing_fk
        REFERENCES public.consignment_listings(id) ON DELETE RESTRICT,
    position_id uuid NOT NULL CONSTRAINT c7_relist_position_fk
        REFERENCES public.consignment_custody_positions(id) ON DELETE RESTRICT,
    mode text NOT NULL,
    auto_relist_enabled boolean NOT NULL,
    config_digest bytea NOT NULL,
    outcome text NOT NULL,
    new_generation bigint,
    command_id uuid NOT NULL CONSTRAINT c7_relist_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    decided_at timestamptz NOT NULL,
    CONSTRAINT c7_relist_generation_fk FOREIGN KEY (position_id, new_generation)
        REFERENCES public.consignment_disposition_generations (position_id, generation)
        ON DELETE RESTRICT,
    CONSTRAINT c7_relist_id_ck CHECK (public.c4_valid_uuid(decision_id)),
    CONSTRAINT c7_relist_mode_ck CHECK (mode IN ('manual', 'auto')),
    -- 自动重列只在冻结的开关为开时才能作出。
    CONSTRAINT c7_relist_auto_ck CHECK ((mode = 'manual' OR auto_relist_enabled) IS TRUE),
    CONSTRAINT c7_relist_digest_ck CHECK (public.c4_valid_digest(config_digest)),
    CONSTRAINT c7_relist_outcome_ck CHECK (outcome IN ('relisted', 'held')),
    CONSTRAINT c7_relist_generation_shape_ck CHECK (
        ((outcome = 'relisted') = (new_generation IS NOT NULL)) IS TRUE
    ),
    CONSTRAINT c7_relist_time_ck CHECK (isfinite(decided_at)),
    -- 未付收回处置：每个处置只作一次决定。
    CONSTRAINT c7_relist_source_uq UNIQUE (source_consumption_id)
);

-- ⑦ 追加事实：除 outbox 的租约列外，本期全部表拒 UPDATE、DELETE 与 TRUNCATE（不变量 8）。
DO $block$
DECLARE
    table_name text;
BEGIN
    FOREACH table_name IN ARRAY ARRAY[
        'consignment_loss_proofs',
        'consignment_loss_facts',
        'consignment_loss_cases',
        'consignment_loss_case_outcomes',
        'consignment_loss_receipts',
        'consignment_rights',
        'consignment_right_states',
        'consignment_right_challenges',
        'consignment_right_challenge_attempts',
        'consignment_right_sessions',
        'consignment_right_session_tokens',
        'consignment_right_notice_results',
        'consignment_right_request_hits',
        'consignment_payment_reminders',
        'consignment_relist_decisions'
    ] LOOP
        EXECUTE format(
            'CREATE TRIGGER %I BEFORE UPDATE OR DELETE ON public.%I
             FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation()',
            'c7_' || table_name || '_append_only', table_name
        );
        EXECUTE format(
            'CREATE TRIGGER %I BEFORE TRUNCATE ON public.%I
             FOR EACH STATEMENT EXECUTE FUNCTION public.consignment_reject_fact_mutation()',
            'c7_' || table_name || '_no_truncate', table_name
        );
    END LOOP;
END
$block$;

-- ⑧ 既有合同的权利回填（R5-C6）：买方权与卖方权按 v5(contract, role) 确定性登记。
-- 定位符摘要取不可逆的占位值（无任何合法定位符与之对应），持有人经本人页或冻结邮箱重新申请；
-- 状态版本 1 为 active 且动作范围为空：失败关闭，由 7D-6 按真实合同状态追加新版本后才有可执行动作。
-- 角色到权利种类：Sale 买家 continue_order、Sale 卖家 claim_payment、Withdrawal 卖家 claim_equipment。
INSERT INTO public.consignment_rights (
    right_id, locator_digest, beneficiary_user_id, frozen_identity, world_id, kind,
    contract_id, position_id, created_command_id, created_at
)
SELECT public.c7_right_id(contract.contract_id, role.role),
       sha256(convert_to('auction.consignment.right.backfill-locator.v1', 'UTF8') || '\x00'::bytea
              || uuid_send(public.c7_right_id(contract.contract_id, role.role))),
       role.beneficiary_user_id, role.frozen_identity, contract.world_id, role.kind,
       contract.contract_id, NULL, contract.origin_command_id, contract.created_at
FROM public.consignment_contracts AS contract
CROSS JOIN LATERAL (
    SELECT 'buyer'::text, sale.buyer_user_id, sale.frozen_buyer_identity, 'continue_order'::text
    FROM public.consignment_sales AS sale
    WHERE sale.contract_id = contract.contract_id
    UNION ALL
    SELECT 'seller'::text, contract.seller_user_id, contract.frozen_seller_identity,
           CASE contract.kind WHEN 'sale' THEN 'claim_payment' ELSE 'claim_equipment' END
) AS role (role, beneficiary_user_id, frozen_identity, kind);

INSERT INTO public.consignment_right_states (right_id, version, state, allowed_actions, occurred_at)
SELECT right_row.right_id, 1, 'active', ARRAY[]::text[], right_row.created_at
FROM public.consignment_rights AS right_row;

-- ======================================================================================================
-- 7M-2：共享替换段（R6 §3.4 ②–⑩，J1 的“串行追加”做法）。以下整函数 / 整约束均以 000136 的最终文本为底，
-- 与底本的差异只有各节注明的新增分支；授权重放与兼容登记仍是文件的最后两条语句。
-- ======================================================================================================

-- ① 命令账本闭集。受限权利的浏览器与权利人是第五、六种 actor（J5），命令仍进同一张全局账本，以保持
-- “同 ID 跨 actor / 操作冲突”的统一仲裁（不变量 1）；两种 actor 的命令 ID 都是玩家侧 UUIDv4。
-- 新操作 relist、confirm_position_loss、record_loss_case_outcome、record_position_loss_proof、
-- fulfill_withdraw_intent 与四个权利操作；新目标 position、right、browser；新结果码 challenge_invalid、
-- right_expired（其余三个 7C-1 拒绝码在 000136 已收）。回执形状见 c4_valid_v2_receipt。
-- 直接撤回回执形状（withdraw 与 fulfill_withdraw_intent 共用）：Withdrawal 合同与装备必填，不带意愿。
CREATE FUNCTION public.c7_valid_withdrawal_accepted(body jsonb) RETURNS boolean
LANGUAGE sql STABLE PARALLEL SAFE SET search_path = pg_catalog, public
AS $function$
    SELECT COALESCE(
        public.c4_valid_json_uuid(body->'listing_id')
        AND public.c4_valid_json_version(body->'listing_version')
        AND body->>'state' = 'withdrawal_pending'
        AND public.c4_valid_json_uuid(body->'contract_id')
        AND public.c4_valid_json_uuid(body->'equipment_id')
        AND NOT body ? 'intent_id', false)
$function$;

-- 本事务写入的损失事实已把本挂单的 position 推到 lost（listing 关闭为 loss_confirmed 的前置）。
CREATE FUNCTION public.c7_listing_loss_in_xact(checked_listing uuid) RETURNS boolean
LANGUAGE sql STABLE PARALLEL SAFE SET search_path = pg_catalog, public
AS $function$
    SELECT EXISTS (
        SELECT 1 FROM public.consignment_loss_facts AS fact
        JOIN public.consignment_custody_positions AS position_row
          ON position_row.id = fact.position_id
        WHERE fact.listing_id = checked_listing
          AND fact.xmin = pg_current_xact_id()::xid
          AND position_row.state = 'lost'
          AND position_row.xmin = pg_current_xact_id()::xid
          AND position_row.current_generation = fact.generation
    )
$function$;

-- 本事务写入的 consignment_withdrawn 交接边（已证原卖家领取）指向本挂单的 position。
CREATE FUNCTION public.c7_listing_withdrawn_in_xact(checked_listing uuid) RETURNS boolean
LANGUAGE sql STABLE PARALLEL SAFE SET search_path = pg_catalog, public
AS $function$
    SELECT EXISTS (
        SELECT 1 FROM public.consignment_equipment_handoffs AS edge
        JOIN public.consignment_custody_positions AS position_row
          ON position_row.id = edge.position_id
        WHERE position_row.listing_id = checked_listing
          AND edge.kind = 'consignment_withdrawn'
          AND edge.xmin = pg_current_xact_id()::xid
    )
$function$;
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
        END
    ) IS TRUE);

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
                'receipt_key', 'buyer_cycle', 'position', 'right', 'browser')
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
                    -- 浏览器目标（000137，J5）没有 UUID 目标，只有 request_target 上的 64 位十六进制绑定摘要。
                    WHEN 'browser' THEN target_id IS NULL AND target_bot_uid IS NULL
                        AND target_world_id IS NULL AND target_external_mid IS NULL
                        AND target_season_id IS NULL AND target_calendar_version IS NULL
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
                    'issue_reservation', 'withdraw', 'purchase', 'reprice', 'relist')
                    AND public.consignment_positive_uuid(actor_ref)
                WHEN 'admin' THEN operation IN (
                    'recheck_quarantine', 'release_quarantine', 'admin_delist', 'release_hold',
                    'confirm_position_loss', 'record_loss_case_outcome')
                    AND public.consignment_valid_text(actor_ref, 256) AND strpos(actor_ref, ':') = 0
                WHEN 'worker' THEN operation IN (
                    'expire_reservation', 'decide_envelope', 'append_quarantine_observation',
                    'start_disposition_write', 'apply_mail_proof', 'season_notice', 'season_close',
                    'authorize_dispatch', 'start_attempt', 'mark_write_started', 'record_evidence',
                    'acquire_return', 'start_return_take', 'consume_return',
                    'register_opening_attribution', 'certify_cod', 'prove_handoff', 'settle_cash',
                    'convert_refund', 'open_sanction', 'record_position_loss_proof',
                    'fulfill_withdraw_intent')
                    AND public.consignment_valid_text(actor_ref, 256)
                WHEN 'system' THEN operation IN (
                    'release_quarantine', 'recheck_quarantine', 'acquire_return')
                    AND public.consignment_valid_text(actor_ref, 256)
                -- 受限权利的浏览器与权利人（000137，J5）：actor_ref 分别是 64 位小写十六进制的浏览器绑定摘要
                -- 与 right_id 文本；二者没有登录账号，命令 ID 仍是 UUIDv4（见 consignment_command_uuid_version）。
                WHEN 'browser' THEN operation IN ('locate_right', 'challenge_right', 'verify_right')
                    AND actor_ref COLLATE "C" ~ '^[0-9a-f]{64}$'
                WHEN 'right_holder' THEN operation = 'claim_right'
                    AND public.consignment_positive_uuid(actor_ref)
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
                                      'purchase', 'reprice', 'relist',
                                      'fulfill_withdraw_intent'))
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
                OR (target_kind = 'position'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation IN ('confirm_position_loss', 'record_loss_case_outcome',
                                      'record_position_loss_proof'))
                OR (target_kind = 'right'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation = 'claim_right')
                OR (target_kind = 'browser'
                    AND request_target COLLATE "C" ~ '^[0-9a-f]{64}$' AND listing_id IS NULL
                    AND operation IN ('locate_right', 'challenge_right', 'verify_right'))
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
                    'participant_ineligible', 'bot_party', 'action_not_allowed', 'self_purchase',
                    'challenge_invalid'))
                OR (result_status = 404 AND result_code IN ('consignment_disabled', 'listing_not_found'))
                OR (result_status = 409 AND result_code IN (
                    'stale_version', 'listing_not_active', 'equipment_busy', 'equipment_committed',
                    'custody_unproven', 'operations_hold', 'season_closed', 'hold_mismatch',
                    'source_mismatch', 'evidence_unproven', 'evidence_conflict',
                    'disposition_conflict', 'fence_mismatch', 'price_changed', 'funding_boundary',
                    'opposite_payment_open', 'lease_lost', 'right_expired'))
                OR (result_status = 429 AND result_code = 'listing_limit_reached')
                OR (result_status = 503 AND result_code IN ('mail_unavailable', 'funding_unavailable'))
            )
        )
    );

-- 回执形状：整函数以 000136 为底，新增 withdraw 的 withdrawal_accepted 处置、拒绝码 challenge_invalid /
-- right_expired 与上述九个操作的分支（缺字段一律失败关闭）。
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
                'participant_ineligible', 'bot_party', 'action_not_allowed', 'self_purchase',
                'challenge_invalid'))
            OR (status = 404 AND body->>'code' IN ('consignment_disabled', 'listing_not_found'))
            OR (status = 409 AND body->>'code' IN (
                'stale_version', 'listing_not_active', 'equipment_busy', 'equipment_committed',
                'custody_unproven', 'operations_hold', 'season_closed', 'hold_mismatch',
                'source_mismatch', 'evidence_unproven', 'evidence_conflict',
                'disposition_conflict', 'fence_mismatch', 'price_changed', 'funding_boundary',
                'opposite_payment_open', 'lease_lost', 'right_expired'))
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
            -- 直接撤回回执（000137）：Withdrawal 合同已同事务建立，回执带合同与装备、不带意愿。
            IF disposition = 'withdrawal_accepted' THEN
                RETURN status = 202 AND public.c7_valid_withdrawal_accepted(body);
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
        -- ===== 以下为 000137 新增分支（与底本其余部分逐字相同）。COALESCE：缺字段时 STRICT 函数返回 NULL，
        -- 而 CHECK 会放过 NULL，新分支一律失败关闭。
        WHEN 'relist' THEN
            RETURN COALESCE(status = 200 AND disposition = 'relisted'
                AND public.c4_valid_json_uuid(body->'listing_id')
                AND public.c4_valid_json_version(body->'listing_version')
                AND body->>'state' = 'available'
                AND public.c6_valid_json_positive(body->'price')
                AND public.c4_valid_json_version(body->'generation'), false);
        WHEN 'fulfill_withdraw_intent' THEN
            RETURN status = 202 AND disposition = 'withdrawal_accepted'
                AND public.c7_valid_withdrawal_accepted(body);
        WHEN 'confirm_position_loss' THEN
            -- 无合同、退款、赔款或新代字段：损失终局不产生任何资金或处分代（D9 §5.3）。
            RETURN COALESCE(status = 200 AND disposition = 'position_loss_recorded'
                AND public.c4_valid_json_uuid(body->'listing_id')
                AND public.c4_valid_json_version(body->'listing_version')
                AND body->>'state' = 'closed'
                AND public.c4_valid_json_uuid(body->'position_id')
                AND public.c4_valid_json_version(body->'position_version')
                AND public.c4_valid_json_uuid(body->'loss_fact_id')
                AND public.c4_valid_json_uuid(body->'loss_case_id')
                AND public.c4_valid_json_uuid(body->'seller_receipt_id')
                AND NOT body ?| ARRAY['contract_id', 'obligation_id', 'refund_id', 'amount',
                                      'generation'], false);
        WHEN 'record_loss_case_outcome' THEN
            RETURN COALESCE(status = 200 AND disposition = 'case_outcome_recorded'
                AND public.c4_valid_json_uuid(body->'case_id')
                AND public.c4_valid_json_version(body->'case_version')
                AND public.c4_valid_json_uuid(body->'outcome_id')
                AND public.c4_valid_json_uuid(body->'seller_receipt_id')
                AND NOT body ?| ARRAY['amount', 'refund_id', 'payment_id', 'issue_id'], false);
        WHEN 'record_position_loss_proof' THEN
            RETURN COALESCE(status = 200 AND disposition = 'loss_proof_recorded'
                AND public.c4_valid_json_uuid(body->'position_id')
                AND public.c4_valid_json_uuid(body->'proof_id')
                AND public.c4_valid_json_digest(body->'proof_digest'), false);
        WHEN 'locate_right', 'challenge_right' THEN
            RETURN COALESCE(status = 202 AND disposition = 'request_received'
                AND public.c4_valid_json_uuid(body->'challenge_id')
                AND public.c4_valid_json_time(body->'expires_at'), false);
        WHEN 'verify_right' THEN
            RETURN COALESCE(status = 200 AND disposition = 'right_verified'
                AND public.c4_valid_json_uuid(body->'challenge_id')
                AND public.c4_valid_json_uuid(body->'right_id')
                AND public.c4_valid_json_version(body->'right_version')
                AND public.c4_valid_json_time(body->'expires_at'), false);
        WHEN 'claim_right' THEN
            RETURN COALESCE(
                ((status = 202 AND disposition = 'claim_accepted')
                 OR (status = 200 AND disposition IN ('already_fulfilling', 'already_completed')))
                AND public.c4_valid_json_uuid(body->'right_id')
                AND public.c4_valid_json_version(body->'right_version'), false);
        ELSE
            RETURN false;
    END CASE;
END
$function$;
-- ② 挂单迁移：整函数以 000136 为底，新增 held_off_market -> available（重列）、available / held_off_market
-- -> closed(loss_confirmed)、withdrawal_pending -> closed(withdrawn_delivered)，并移除末尾对
-- loss_confirmed / withdrawn_delivered 的恒拒。每条边只在同事务事实齐备时放行（不变量 2、3、6）。
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
               OR NEW.reason_code IS NULL
               OR NEW.reason_code NOT IN ('season_closed', 'loss_confirmed')
               OR (NEW.reason_code = 'loss_confirmed'
                   AND NOT public.c7_listing_loss_in_xact(NEW.id)) THEN
                -- 已证灭失（000137）：同事务的损失事实已把 position 推到 lost；事件与命令由事件核对绑定。
                RAISE EXCEPTION '寄售可售态关闭只能因季末或已证灭失，且不能改价、快照或期限'
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
        ELSIF NEW.state = 'available' THEN
            -- 重列（000137）：价格、快照与期限不变且清空下架原因；同事务的重列命令、g+1 重列代与
            -- relisted 事件由 consignment_check_listing_event 逐项绑定（不变量 2、3）。
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at
               OR NEW.reason_code IS NOT NULL THEN
                RAISE EXCEPTION '寄售重列不能改价、快照或期限，且必须清空下架原因'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSIF NEW.state = 'closed' THEN
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at
               OR NEW.reason_code IS NULL
               OR NEW.reason_code NOT IN ('season_closed', 'loss_confirmed')
               OR (NEW.reason_code = 'loss_confirmed'
                   AND NOT public.c7_listing_loss_in_xact(NEW.id)) THEN
                RAISE EXCEPTION '寄售已下架态关闭只能因季末或已证灭失，且不能改价、快照或期限'
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
               OR NEW.reason_code IS NULL
               OR NEW.reason_code NOT IN ('season_closed', 'withdrawn_delivered')
               OR (NEW.reason_code = 'withdrawn_delivered'
                   AND NOT public.c7_listing_withdrawn_in_xact(NEW.id)) THEN
                -- 已证原卖家领取（000137）：同事务必须写入 consignment_withdrawn 交接边。
                RAISE EXCEPTION '寄售撤回态关闭只能因季末或已证原卖家领取，且不能改价、快照或期限'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSE
            RAISE EXCEPTION '寄售撤回态不能迁移到该状态'
                USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
        END IF;
    END IF;

    -- 000137 起 withdrawn_delivered 与 loss_confirmed 由上面各自的分支放行并绑定同事务事实，不再有恒拒尾段。
    RETURN NEW;
END
$function$;
-- ③ 挂单版本的同事务事件核对：整函数以 000136 为底，新增 relisted / withdrawal_accepted / loss_confirmed /
-- withdrawn_delivered 四个 CASE，各自绑定同事务命令与对应事实；事件边闭集与回执 -> 事件的延迟触发器同步放开。
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
                    -- ===== 以下四个事件为 000137 新增（重列、直接撤回、灭失、撤回交付）。 =====
                    WHEN 'relisted' THEN
                        c.operation = 'relist' AND c.actor_kind = 'public_user'
                        AND c.listing_id = NEW.id AND c.actor_ref = NEW.seller_user_id::text
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
                    WHEN 'relisted' THEN actor_kind = 'public_user' AND to_state = 'available'
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
                END
            WHEN from_state = 'withdrawal_pending' THEN
                CASE event_kind
                    WHEN 'withdrawn_delivered' THEN actor_kind = 'worker' AND to_state = 'closed'
                        AND reason_code = 'withdrawn_delivered'
                    WHEN 'season_closed' THEN actor_kind IN ('worker', 'system')
                        AND to_state = 'closed' AND reason_code = 'season_closed'
                END
        END
    ) IS TRUE);
-- 成功回执必须与同版本事件同事务：新增 relisted、withdrawal_accepted。
DROP TRIGGER consignment_command_version_event ON public.consignment_commands;
CREATE CONSTRAINT TRIGGER consignment_command_version_event
AFTER INSERT ON public.consignment_commands DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW
WHEN (NEW.result_status < 400 AND NEW.result_body->>'disposition' IN
    ('registered', 'repriced', 'closed', 'published', 'rejected', 'expired',
     'reservation_issued', 'hold_set', 'hold_released', 'purchased', 'relisted',
     'withdrawal_accepted'))
EXECUTE FUNCTION public.consignment_check_command_event();
-- ④ 处分代：放开 g+1。首代仍是 first_sale；g+1 只能是 relist（重列）或 withdrawal_only（直接撤回），
-- predecessor = generation - 1 由 c4_generation_number_ck 保证，唯一后继由 c4_generation_successor_uq 保证
-- （跳代与重复后继被拒，不变量 2、6）。c4_guard_generation 仍在行级核对前驱存在；“后继须同事务命令 / 合同”
-- 因合同行晚于代行写入，改由下面的延迟触发器在 COMMIT 前核对。
ALTER TABLE public.consignment_disposition_generations
    DROP CONSTRAINT c4_generation_phase4_ck,
    ADD CONSTRAINT c7_generation_purpose_ck CHECK (
        (generation = 1 AND purpose = 'first_sale')
        OR (generation > 1 AND purpose IN ('relist', 'withdrawal_only'))
    );

-- ⑧ Withdrawal 链路字段（J4）：撤回代的前驱代序号与上代封闭证明引用；R4 的合同表第 7 期才有生产行，零行变更。
-- 二者同空同非空：无新代的直接撤回（当前代未授合同）两列皆空。
ALTER TABLE public.consignment_withdrawals
    ADD COLUMN predecessor_generation bigint,
    ADD COLUMN closure_proof_ref text,
    ADD CONSTRAINT c7_withdrawal_generation_ck CHECK (
        predecessor_generation IS NULL OR predecessor_generation BETWEEN 1 AND 9007199254740991
    ),
    ADD CONSTRAINT c7_withdrawal_closure_ck CHECK (
        ((predecessor_generation IS NULL) = (closure_proof_ref IS NULL))
        AND (closure_proof_ref IS NULL OR public.consignment_safe_observation_text(closure_proof_ref, 256))
    );

-- 撤回合同的前驱代必须恰是其所在代的前驱，且所在代是 withdrawal_only 代。
CREATE FUNCTION public.c7_guard_withdrawal_chain() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NEW.predecessor_generation IS NULL THEN
        RETURN NEW;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM public.consignment_contracts AS contract
        JOIN public.consignment_disposition_generations AS generation_row
          ON generation_row.position_id = contract.position_id
         AND generation_row.generation = contract.generation
        WHERE contract.contract_id = NEW.contract_id
          AND generation_row.purpose = 'withdrawal_only'
          AND generation_row.predecessor = NEW.predecessor_generation
    ) THEN
        RAISE EXCEPTION 'Withdrawal 的前驱代必须是其所在 withdrawal_only 代的前驱'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_withdrawal_chain';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c7_withdrawal_chain
BEFORE INSERT ON public.consignment_withdrawals
FOR EACH ROW EXECUTE FUNCTION public.c7_guard_withdrawal_chain();

-- g+1 后继必须带同事务的命令（与合同）：relist 代对应成功的 relist 命令；withdrawal_only 代对应成功的
-- withdraw / fulfill_withdraw_intent 命令，且同事务建立挂在该代上的 Withdrawal 合同并带前驱代与封闭证明引用。
-- 延迟到 COMMIT：命令行与合同行都可能晚于代行写入（不变量 2、3）。
CREATE FUNCTION public.c7_check_generation_successor() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NEW.generation = 1 THEN
        RETURN NULL;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM public.consignment_commands AS command_row
        WHERE command_row.command_id = NEW.created_command_id
          AND command_row.xmin = pg_current_xact_id()::xid
          AND command_row.result_status < 400
          AND CASE NEW.purpose
              WHEN 'relist' THEN command_row.operation = 'relist'
                  AND command_row.actor_kind = 'public_user'
              WHEN 'withdrawal_only' THEN
                  (command_row.operation = 'withdraw' AND command_row.actor_kind = 'public_user')
                  OR (command_row.operation = 'fulfill_withdraw_intent'
                      AND command_row.actor_kind = 'worker')
              ELSE false END
    ) THEN
        RAISE EXCEPTION '处分代后继缺少同事务的成功命令'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_generation_successor_command';
    END IF;
    IF NEW.purpose = 'withdrawal_only' AND NOT EXISTS (
        SELECT 1 FROM public.consignment_contracts AS contract
        JOIN public.consignment_withdrawals AS withdrawal
          ON withdrawal.contract_id = contract.contract_id
        WHERE contract.position_id = NEW.position_id AND contract.generation = NEW.generation
          AND contract.kind = 'withdrawal' AND contract.xmin = pg_current_xact_id()::xid
          AND contract.origin_command_id = NEW.created_command_id
          AND withdrawal.predecessor_generation = NEW.predecessor
    ) THEN
        RAISE EXCEPTION '撤回代缺少同事务的 Withdrawal 合同与前驱代引用'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_generation_successor_contract';
    END IF;
    -- 当前指针 CAS（D9 §5.3 撤回 3）：同事务内 position 的 current_generation 必须恰好前进到本代。
    IF NOT EXISTS (
        SELECT 1 FROM public.consignment_custody_positions AS position_row
        WHERE position_row.id = NEW.position_id
          AND position_row.current_generation = NEW.generation
          AND position_row.xmin = pg_current_xact_id()::xid
    ) THEN
        RAISE EXCEPTION '处分代后继必须同事务把 position 的当前指针推进到本代'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_generation_successor_pointer';
    END IF;
    RETURN NULL;
END
$function$;
CREATE CONSTRAINT TRIGGER c7_generation_successor
AFTER INSERT ON public.consignment_disposition_generations
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.c7_check_generation_successor();

-- ⑤ position 状态机：整函数以 000136 为底，新增 lost 与 committed / in_transit / reconcile -> season_ended。
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
    IF NEW.state = OLD.state AND NEW.current_generation IS DISTINCT FROM OLD.current_generation THEN
        -- 处分代指针换代（000137，D9 §5.3 撤回 3 / 重列）：状态不变，指针只能前进到 g+1，且同事务写入了
        -- (position, g+1) 代行（predecessor=g，purpose 为 relist / withdrawal_only）及其成功命令；
        -- 跳代、回退与没有后继行一律拒绝（不变量 2、6）。只在原物已取得或收回的 in_custody / committed 放行
        -- （D9 §5.3 撤回 2：旧买方交割已封闭）；in_transit、reconcile 的交割未封闭或未知，不得换代。
        IF OLD.state NOT IN ('in_custody', 'committed')
           OR OLD.current_generation IS NULL
           OR NEW.current_generation IS DISTINCT FROM OLD.current_generation + 1
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_disposition_generations AS successor
               JOIN public.consignment_commands AS command_row
                 ON command_row.command_id = successor.created_command_id
               WHERE successor.position_id = NEW.id
                 AND successor.generation = NEW.current_generation
                 AND successor.predecessor = OLD.current_generation
                 AND successor.purpose IN ('relist', 'withdrawal_only')
                 AND successor.xmin = pg_current_xact_id()::xid
                 AND command_row.xmin = pg_current_xact_id()::xid
                 AND command_row.result_status < 400
           ) THEN
            RAISE EXCEPTION '处分代指针只能前进到同事务写入的唯一后继代（g+1）'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_position_generation_pointer';
        END IF;
    ELSIF OLD.state = 'awaiting_take' AND NEW.state = 'in_custody' THEN
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
    ELSIF OLD.state IN ('in_custody', 'committed', 'in_transit', 'reconcile')
          AND NEW.state = 'season_ended' THEN
        -- 已承诺、在途与核对中的件同样在季窗结束后终结（000137，J8）；退款与交付责任不在此处理。
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
    ELSIF OLD.state IN ('in_custody', 'committed', 'in_transit', 'reconcile')
          AND NEW.state = 'lost' THEN
        -- 已证灭失（000137，J8）：指针留在确认时的当前代（D9 §5.3，不新增处分代），且同事务必须写入
        -- 绑定本 position 与当前代的损失事实；损失事实的外键保证它引用同 position 的损失证据。
        IF NEW.current_generation IS DISTINCT FROM OLD.current_generation
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_loss_facts AS fact
               WHERE fact.position_id = NEW.id AND fact.generation = NEW.current_generation
                 AND fact.xmin = pg_current_xact_id()::xid
           ) THEN
            RAISE EXCEPTION '灭失必须同事务写入当前处分代的损失事实且不替换处分代'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_position_loss_fact';
        END IF;
    ELSE
        RAISE EXCEPTION 'position 迁移不合法'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_position_transition';
    END IF;
    RETURN NEW;
END
$function$;
-- 代管事件：放开 lost（proof_id 取损失证据，列上仍无外键），season_ended 的来源态加入 committed / in_transit；
-- lost 的来源态限于仍占用实物的四态。
ALTER TABLE public.consignment_custody_events
    DROP CONSTRAINT c6_custody_event_kinds_ck,
    DROP CONSTRAINT c6_custody_event_season_ended_ck,
    DROP CONSTRAINT c4_custody_event_state_ck,
    ADD CONSTRAINT c7_custody_event_kinds_ck CHECK (
        event_kind IN ('accepted', 'in_custody', 'reconcile', 'committed', 'in_transit',
                       'handed_over', 'season_ended', 'lost')
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
            WHEN 'lost' THEN from_state IN ('in_custody', 'committed', 'in_transit', 'reconcile')
                AND to_state = 'lost'
        END
    ) IS TRUE),
    ADD CONSTRAINT c7_custody_event_season_ended_ck CHECK (
        event_kind <> 'season_ended'
        OR (from_state IN ('awaiting_take', 'in_custody', 'committed', 'in_transit', 'reconcile')
            AND proof_id IS NULL)
    );

-- 代管事件的证明引用：lost 事件的 proof_id 是损失证据（consignment_loss_proofs），其余事件仍是邮件证明
-- （mail_disposition_proofs）。同一列无法同时挂两个外键，原外键 c4_custody_event_proof_fk 改为延迟触发器，
-- 在 COMMIT 前按事件种类核对被引用的证明确实存在（lost 还须属于同一 position），语义与原延迟外键相同：
-- 触发器同时覆盖 INSERT 与 UPDATE OF proof_id / event_kind / position_id（原外键对改写 proof_id 同样生效），
-- 被引用的一侧（mail_disposition_proofs 与 consignment_loss_proofs）另有立即触发器，保持原 ON DELETE
-- RESTRICT 的效果：被引用时不可删除或改键（不变量 3、8）。代管事件与两张证明表本身另受追加表触发器保护，
-- 这里是不依赖该保护的第二道约束。
ALTER TABLE public.consignment_custody_events
    DROP CONSTRAINT c4_custody_event_proof_fk;
CREATE FUNCTION public.c7_check_custody_event_proof() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NEW.event_kind = 'lost' THEN
        IF NOT EXISTS (
            SELECT 1 FROM public.consignment_loss_proofs AS proof
            WHERE proof.proof_id = NEW.proof_id AND proof.position_id = NEW.position_id
        ) THEN
            RAISE EXCEPTION 'lost 事件必须引用同 position 的损失证据'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_custody_event_loss_proof';
        END IF;
    ELSIF NOT EXISTS (
        SELECT 1 FROM public.mail_disposition_proofs AS proof WHERE proof.id = NEW.proof_id
    ) THEN
        RAISE EXCEPTION '代管事件必须引用已存在的邮件证明'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_custody_event_mail_proof';
    END IF;
    RETURN NULL;
END
$function$;
CREATE CONSTRAINT TRIGGER c7_custody_event_proof
AFTER INSERT OR UPDATE OF proof_id, event_kind, position_id ON public.consignment_custody_events
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW WHEN (NEW.proof_id IS NOT NULL)
EXECUTE FUNCTION public.c7_check_custody_event_proof();

-- 被引用一侧：邮件证明被非 lost 事件引用、损失证据被 lost 事件引用时，不可删除或改键（原外键 RESTRICT）。
CREATE FUNCTION public.c7_guard_custody_proof_referenced() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    old_key uuid;
    lost_events boolean;
BEGIN
    IF TG_TABLE_NAME = 'consignment_loss_proofs' THEN
        old_key := OLD.proof_id;
        lost_events := true;
    ELSE
        old_key := OLD.id;
        lost_events := false;
    END IF;
    IF EXISTS (
        SELECT 1 FROM public.consignment_custody_events AS event_row
        WHERE event_row.proof_id = old_key AND (event_row.event_kind = 'lost') = lost_events
    ) THEN
        RAISE EXCEPTION '被代管事件引用的证明不可删除或改键'
            USING ERRCODE = '23503', CONSTRAINT = 'c7_custody_event_proof_ref';
    END IF;
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c7_custody_mail_proof_ref
BEFORE DELETE OR UPDATE OF id ON public.mail_disposition_proofs
FOR EACH ROW EXECUTE FUNCTION public.c7_guard_custody_proof_referenced();
CREATE TRIGGER c7_custody_loss_proof_ref
BEFORE DELETE OR UPDATE OF proof_id ON public.consignment_loss_proofs
FOR EACH ROW EXECUTE FUNCTION public.c7_guard_custody_proof_referenced();

-- ⑥ 占用释放守卫：整函数以 000136 为底，只新增 lost 分支（认同事务的损失事实与证据）；
-- handed_over 仍认同事务的确领证明，其余终止事件仍认同事务的邮件证明。
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
        -- 季窗已结束（等号即失效）”证明；handed_over 在 000136 改认确领证明；lost 在 000137 改认损失事实。
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
        -- 灭失（000137，J8）：占用释放的 last_fact 是本事务写入的损失事实，lost 事件的 proof_id 是该事实
        -- 引用的损失证据（不是邮件证明）；没有损失事实或证据对不上一律拒绝。handed_over 的证明要求不变。
        IF event_row.event_kind = 'lost' THEN
            IF event_row.proof_id IS NULL
               OR event_row.handoff_proof_id IS NOT NULL
               OR position_row.id IS NULL
               OR position_row.state <> 'lost'
               OR position_row.xmin <> pg_current_xact_id()::xid
               OR NOT EXISTS (
                   SELECT 1 FROM public.consignment_loss_facts AS fact
                   JOIN public.consignment_loss_proofs AS proof
                     ON proof.proof_id = fact.proof_id AND proof.position_id = fact.position_id
                   WHERE fact.position_id = old_position
                     AND fact.loss_fact_id = NEW.last_fact_id
                     AND fact.proof_id = event_row.proof_id
                     AND fact.xmin = pg_current_xact_id()::xid
               ) THEN
                RAISE EXCEPTION '灭失释放 position 缺少本事务的 lost 事件、损失事实与损失证据'
                    USING ERRCODE = '23514', CONSTRAINT = 'c7_claim_loss_fact';
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
-- ⑦ 交接边：放开 consignment_withdrawn（已证原卖家领取），与 consignment_delivered 同样必须来自 position。
ALTER TABLE public.consignment_equipment_handoffs
    DROP CONSTRAINT c6_handoff_edge_kinds_ck,
    DROP CONSTRAINT c6_handoff_edge_position_kind_ck,
    ADD CONSTRAINT c7_handoff_edge_kinds_ck CHECK (kind IN (
        'custody_acquired', 'auction_winner_collected', 'auction_unsold_returned',
        'auction_withdrawn_returned', 'consignment_delivered', 'consignment_withdrawn'
    )),
    ADD CONSTRAINT c7_handoff_edge_position_kind_ck CHECK (
        kind NOT IN ('consignment_delivered', 'consignment_withdrawn') OR position_id IS NOT NULL
    );

-- 损失命令的同事务事实（原子性，不变量 3）：成功的灭失确认 / 证据 / 案件结果回执必须与它们写下的事实
-- 同事务提交，不存在“回执在、事实缺”。
CREATE FUNCTION public.c7_check_command_loss_fact() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NEW.operation = 'confirm_position_loss' THEN
        IF NOT EXISTS (
            SELECT 1 FROM public.consignment_loss_facts AS fact
            JOIN public.consignment_loss_cases AS case_row
              ON case_row.loss_fact_id = fact.loss_fact_id
            WHERE fact.loss_fact_id = (NEW.result_body->>'loss_fact_id')::uuid
              AND fact.position_id = NEW.target_id AND fact.origin_command_id = NEW.command_id
              AND case_row.case_id = (NEW.result_body->>'loss_case_id')::uuid
              AND fact.xmin = pg_current_xact_id()::xid
        ) THEN
            RAISE EXCEPTION '灭失确认回执缺少同事务的损失事实与案件'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_command_loss_fact';
        END IF;
    ELSIF NEW.operation = 'record_position_loss_proof' THEN
        IF NOT EXISTS (
            SELECT 1 FROM public.consignment_loss_proofs AS proof
            WHERE proof.proof_id = (NEW.result_body->>'proof_id')::uuid
              AND proof.position_id = NEW.target_id AND proof.origin_command_id = NEW.command_id
              AND proof.xmin = pg_current_xact_id()::xid
        ) THEN
            RAISE EXCEPTION '灭失证据回执缺少同事务的证据行'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_command_loss_fact';
        END IF;
    ELSIF NEW.operation = 'record_loss_case_outcome' THEN
        IF NOT EXISTS (
            SELECT 1 FROM public.consignment_loss_case_outcomes AS outcome
            WHERE outcome.outcome_id = (NEW.result_body->>'outcome_id')::uuid
              AND outcome.case_id = (NEW.result_body->>'case_id')::uuid
              AND outcome.command_id = NEW.command_id
              AND outcome.xmin = pg_current_xact_id()::xid
        ) THEN
            RAISE EXCEPTION '案件结果回执缺少同事务的结果行'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_command_loss_fact';
        END IF;
    END IF;
    RETURN NULL;
END
$function$;
CREATE CONSTRAINT TRIGGER c7_command_loss_fact
AFTER INSERT ON public.consignment_commands
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW
WHEN (NEW.result_status < 400
      AND NEW.operation IN ('confirm_position_loss', 'record_position_loss_proof',
                            'record_loss_case_outcome'))
EXECUTE FUNCTION public.c7_check_command_loss_fact();

-- J7 付款提醒走通用 consignment_notice 投递：template 放宽为季前通知（仅 isekai）与付款提醒（main 与 isekai），
-- 其余形状（无附件、无 COD、无 credits、无装备引用）不变。发送类别仍由 origin 归 settlement。
ALTER TABLE public.deliveries
    DROP CONSTRAINT c4_delivery_consignment_notice_payload_ck;
ALTER TABLE public.deliveries
    ADD CONSTRAINT c4_delivery_consignment_notice_payload_ck CHECK (
        (origin_kind <> 'consignment_notice'
        OR (
            payload->'schema_version' = '1'::jsonb
            AND payload->>'operation' = 'send'
            AND (
                (payload->>'template' = 'consignment_season_notice'
                 AND payload->>'world' = 'isekai')
                OR (payload->>'template' = 'consignment_payment_reminder'
                    AND payload->>'world' IN ('main', 'isekai'))
            )
            AND payload->'credits' = '0'::jsonb
            AND payload->'cod' = '0'::jsonb
            AND jsonb_typeof(payload->'attachment_ids') = 'array'
            AND payload->'attachment_ids' = '[]'::jsonb
            AND NOT payload ? 'equipment_secret_refs'
            AND jsonb_typeof(payload->'body') = 'string'
        )) IS TRUE
    );

-- ⑨ 处罚效果与账号工作结果（7B-2 执行器写）。6D3 的处罚与账号工作状态只是投影；本期把“账号效果已落 /
-- 周期已解禁而收口”追加成不可改写的事实。state 与 6D3 处罚的 applied / cycle_closed 同名；账号工作用
-- completed / cancelled，对应关系由延迟触发器核对。cycle_closed 没有账号命令，applied 必有（拉黑命令账本
-- user_admin_commands，同一处罚业务键只对应一条，重放返回原回执，不变量 1、8）。
CREATE TABLE public.consignment_sanction_effects (
    sanction_id uuid NOT NULL CONSTRAINT c7_effect_pk PRIMARY KEY,
    buyer_user_id uuid NOT NULL,
    state text NOT NULL,
    account_command_id uuid CONSTRAINT c7_effect_account_command_fk
        REFERENCES public.user_admin_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    applied_at timestamptz NOT NULL,
    CONSTRAINT c7_effect_sanction_fk FOREIGN KEY (sanction_id, buyer_user_id)
        REFERENCES public.consignment_sanctions (id, buyer_user_id) ON DELETE RESTRICT,
    CONSTRAINT c7_effect_state_ck CHECK (state IN ('applied', 'cycle_closed')),
    CONSTRAINT c7_effect_command_ck CHECK (
        ((state = 'applied') = (account_command_id IS NOT NULL)) IS TRUE
    ),
    CONSTRAINT c7_effect_time_ck CHECK (isfinite(applied_at)),
    CONSTRAINT c7_effect_command_uq UNIQUE (account_command_id)
);

CREATE TABLE public.consignment_sanction_work_results (
    work_id uuid NOT NULL CONSTRAINT c7_work_result_pk PRIMARY KEY
        CONSTRAINT c7_work_result_work_fk
        REFERENCES public.consignment_account_work(id) ON DELETE RESTRICT,
    sanction_id uuid NOT NULL CONSTRAINT c7_work_result_effect_fk
        REFERENCES public.consignment_sanction_effects(sanction_id) ON DELETE RESTRICT,
    result text NOT NULL,
    recorded_at timestamptz NOT NULL,
    CONSTRAINT c7_work_result_ck CHECK (result IN ('applied', 'cycle_closed')),
    CONSTRAINT c7_work_result_time_ck CHECK (isfinite(recorded_at)),
    CONSTRAINT c7_work_result_sanction_uq UNIQUE (sanction_id)
);

-- 效果与结果必须与 6D3 的处罚 / 账号工作投影在同一事务一起前进：处罚状态等于效果状态；applied 的拉黑命令
-- 确属该买家；结果行的账号工作确属该处罚，且工作状态与结果对应（applied -> completed、cycle_closed -> cancelled）。
CREATE FUNCTION public.c7_check_sanction_effect() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF TG_TABLE_NAME = 'consignment_sanction_effects' THEN
        IF NOT EXISTS (
            SELECT 1 FROM public.consignment_sanctions AS sanction
            WHERE sanction.id = NEW.sanction_id AND sanction.state = NEW.state
        ) OR (NEW.state = 'applied' AND NOT EXISTS (
            SELECT 1 FROM public.user_admin_commands AS account_command
            WHERE account_command.command_id = NEW.account_command_id
              AND account_command.command_kind = 'blacklist'
              AND account_command.user_id = NEW.buyer_user_id
        )) THEN
            RAISE EXCEPTION '处罚效果必须与处罚状态一致，且 applied 必带该买家的拉黑命令'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_effect_projection';
        END IF;
    ELSIF NOT EXISTS (
        SELECT 1 FROM public.consignment_account_work AS work
        WHERE work.id = NEW.work_id AND work.sanction_id = NEW.sanction_id
          AND work.state = CASE NEW.result WHEN 'applied' THEN 'completed' ELSE 'cancelled' END
    ) OR NOT EXISTS (
        SELECT 1 FROM public.consignment_sanction_effects AS effect
        WHERE effect.sanction_id = NEW.sanction_id AND effect.state = NEW.result
    ) THEN
        RAISE EXCEPTION '账号工作结果必须与处罚效果和工作状态一致'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_work_result_projection';
    END IF;
    RETURN NULL;
END
$function$;
CREATE CONSTRAINT TRIGGER c7_effect_projection
AFTER INSERT ON public.consignment_sanction_effects
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.c7_check_sanction_effect();
CREATE CONSTRAINT TRIGGER c7_work_result_projection
AFTER INSERT ON public.consignment_sanction_work_results
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.c7_check_sanction_effect();

-- ⑩ 本段新表同样只追加：拒 UPDATE、DELETE 与 TRUNCATE（不变量 8）；授权在文件头的包装函数里。
DO $block$
DECLARE
    table_name text;
BEGIN
    FOREACH table_name IN ARRAY ARRAY[
        'consignment_sanction_effects',
        'consignment_sanction_work_results'
    ] LOOP
        EXECUTE format(
            'CREATE TRIGGER %I BEFORE UPDATE OR DELETE ON public.%I
             FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation()',
            'c7_' || table_name || '_append_only', table_name
        );
        EXECUTE format(
            'CREATE TRIGGER %I BEFORE TRUNCATE ON public.%I
             FOR EACH STATEMENT EXECUTE FUNCTION public.consignment_reject_fact_mutation()',
            'c7_' || table_name || '_no_truncate', table_name
        );
    END LOOP;
END
$block$;

-- ⑪ 7C-3：Sale 封闭事实与 committed -> in_custody 回库边（本节追加在授权重放之前）。
-- D9 §5.1–§5.3 要求撤回 / 重列核对“旧买方交割已封闭、须先完成的退款处置已封闭、原件已取得或收回”，
-- 但 000136 没有任何“Sale 已封闭”的事实表（R6 §5.2 把它留给“合同处置事实”，一直无 owner）。这里追加
-- 只追加的封闭事实：cancelled_unpaid（买家从未付款、从未确领，购买被取消）与 refund_closed（已付款但
-- 退款已由 6B 单写者转换，卖款不会再发）。封闭不等于原件在库：原件是否在库另由 c7_sale_original_back
-- 按派发尝试与退件接管核对。写入方：refund_closed 由 7C-3 退款调用写；cancelled_unpaid 由退回处置的
-- 执行方在取消购买的同事务写（basis_ref 记处置 ID）。
CREATE TABLE public.consignment_sale_closures (
    contract_id uuid NOT NULL CONSTRAINT c7_closure_pk PRIMARY KEY
        CONSTRAINT c7_closure_sale_fk
        REFERENCES public.consignment_sales(contract_id) ON DELETE RESTRICT,
    closure_kind text NOT NULL,
    basis_ref text NOT NULL,
    closed_at timestamptz NOT NULL,
    CONSTRAINT c7_closure_kind_ck CHECK (closure_kind IN ('cancelled_unpaid', 'refund_closed')),
    CONSTRAINT c7_closure_basis_ck CHECK (public.consignment_safe_observation_text(basis_ref, 200)),
    CONSTRAINT c7_closure_time_ck CHECK (isfinite(closed_at))
);

-- 封闭的前置：买家从未确领原件；未付取消要求没有 funded 投影；退款封闭要求已转换为退款（不变量 4、6）。
CREATE FUNCTION public.c7_guard_sale_closure() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF EXISTS (
        SELECT 1 FROM public.consignment_handoff_proofs AS handoff
        JOIN public.settlement_obligations AS obligation ON obligation.id = handoff.obligation_id
        WHERE obligation.contract_id = NEW.contract_id
    ) THEN
        RAISE EXCEPTION '买家已确领原件的 Sale 不能封闭'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_sale_closure_claimed';
    END IF;
    IF NEW.closure_kind = 'cancelled_unpaid' AND EXISTS (
        SELECT 1 FROM public.consignment_funded_sales AS funded
        WHERE funded.contract_id = NEW.contract_id
    ) THEN
        RAISE EXCEPTION '已收款的 Sale 不能按未付取消封闭'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_sale_closure_paid';
    END IF;
    IF NEW.closure_kind = 'refund_closed' AND NOT EXISTS (
        SELECT 1 FROM public.consignment_funded_sales AS funded
        WHERE funded.contract_id = NEW.contract_id AND funded.disposition = 'refund_converted'
    ) THEN
        RAISE EXCEPTION '退款封闭要求该 Sale 已转换为买家退款'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_sale_closure_refund';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c7_sale_closure_binding
BEFORE INSERT ON public.consignment_sale_closures
FOR EACH ROW EXECUTE FUNCTION public.c7_guard_sale_closure();
CREATE TRIGGER c7_consignment_sale_closures_append_only
BEFORE UPDATE OR DELETE ON public.consignment_sale_closures
FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER c7_consignment_sale_closures_no_truncate
BEFORE TRUNCATE ON public.consignment_sale_closures
FOR EACH STATEMENT EXECUTE FUNCTION public.consignment_reject_fact_mutation();

-- 原件在库判定：资产腿（原件 COD / 交接）的每个已起写 Send 都有同轮的精确 Take 消费，且没有尚未消费的
-- 原件退件接管（Take 已起写或待起写的未知）。从未起写的 Sale 两个条件都空成立。读与守卫共用本函数。
CREATE FUNCTION public.c7_sale_original_back(sale_contract uuid) RETURNS boolean
LANGUAGE sql STABLE SET search_path = pg_catalog
AS $function$
    SELECT NOT EXISTS (
        SELECT 1 FROM public.consignment_dispatch_rounds AS round_row
        JOIN public.settlement_obligations AS obligation
          ON obligation.id = round_row.obligation_id
        JOIN public.delivery_send_attempts AS send_attempt
          ON send_attempt.delivery_id = round_row.delivery_id
        WHERE round_row.contract_id = sale_contract
          AND obligation.obligation_kind IN ('winner_equipment_cod', 'winner_equipment_handoff')
          AND NOT EXISTS (
              SELECT 1 FROM public.consignment_return_consumptions AS consumption
              WHERE consumption.contract_id = round_row.contract_id
                AND consumption.obligation_id = round_row.obligation_id
                AND consumption.original_round = round_row.round
          )
    ) AND NOT EXISTS (
        SELECT 1 FROM public.consignment_return_intakes AS intake
        WHERE intake.contract_id = sale_contract AND intake.leg = 'original'
          AND NOT EXISTS (
              SELECT 1 FROM public.consignment_return_consumptions AS consumption
              WHERE consumption.intake_id = intake.id
          )
    )
$function$;

-- 回库事件：新增 uncommitted 种类（proof_id 引用 Sale 封闭事实而不是邮件证明）；不给既有事件表加列，
-- 升级后既有行逐字节不变。
ALTER TABLE public.consignment_custody_events
    DROP CONSTRAINT c4_custody_event_kind_ck,
    DROP CONSTRAINT c7_custody_event_kinds_ck,
    DROP CONSTRAINT c4_custody_event_state_ck,
    ADD CONSTRAINT c4_custody_event_kind_ck CHECK (
        event_kind IN ('accepted', 'in_custody', 'reconcile', 'committed', 'in_transit',
                       'handed_over', 'season_ended', 'lost', 'uncommitted')
    ),
    ADD CONSTRAINT c7_custody_event_kinds_ck CHECK (
        event_kind IN ('accepted', 'in_custody', 'reconcile', 'committed', 'in_transit',
                       'handed_over', 'season_ended', 'lost', 'uncommitted')
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
            WHEN 'lost' THEN from_state IN ('in_custody', 'committed', 'in_transit', 'reconcile')
                AND to_state = 'lost'
            -- 回库：Sale 封闭后原件仍在库，committed 回到 in_custody（000137，7C-3）。
            WHEN 'uncommitted' THEN from_state = 'committed' AND to_state = 'in_custody'
        END
    ) IS TRUE),
    -- 回库事件的 proof_id 是 Sale 封闭事实（consignment_sale_closures.contract_id），不是邮件证明；
    -- 与 lost 事件引用损失证据同一做法，引用的存在性由下面的延迟触发器核对。
    ADD CONSTRAINT c7_custody_event_recall_ck CHECK (
        event_kind <> 'uncommitted' OR public.c4_valid_uuid(proof_id)
    );

CREATE OR REPLACE FUNCTION public.c7_check_custody_event_proof() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NEW.event_kind = 'lost' THEN
        IF NOT EXISTS (
            SELECT 1 FROM public.consignment_loss_proofs AS proof
            WHERE proof.proof_id = NEW.proof_id AND proof.position_id = NEW.position_id
        ) THEN
            RAISE EXCEPTION 'lost 事件必须引用同 position 的损失证据'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_custody_event_loss_proof';
        END IF;
    ELSIF NEW.event_kind = 'uncommitted' THEN
        IF NOT EXISTS (
            SELECT 1 FROM public.consignment_sale_closures AS closure
            JOIN public.consignment_contracts AS contract ON contract.contract_id = closure.contract_id
            WHERE closure.contract_id = NEW.proof_id AND contract.position_id = NEW.position_id
        ) THEN
            RAISE EXCEPTION '回库事件必须引用同 position 的 Sale 封闭事实'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_custody_event_closure_proof';
        END IF;
    ELSIF NOT EXISTS (
        SELECT 1 FROM public.mail_disposition_proofs AS proof WHERE proof.id = NEW.proof_id
    ) THEN
        RAISE EXCEPTION '代管事件必须引用已存在的邮件证明'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_custody_event_mail_proof';
    END IF;
    RETURN NULL;
END
$function$;

-- 回库守卫：整函数以本文件上文的 c4_guard_position 为底，只新增 committed -> in_custody 分支。
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
    IF NEW.state = OLD.state AND NEW.current_generation IS DISTINCT FROM OLD.current_generation THEN
        -- 处分代指针换代（000137，D9 §5.3 撤回 3 / 重列）：状态不变，指针只能前进到 g+1，且同事务写入了
        -- (position, g+1) 代行（predecessor=g，purpose 为 relist / withdrawal_only）；其成功命令与合同由
        -- c7_check_generation_successor 在 COMMIT 前核对（7C-3 修正：命令账本行按 DepositWriter 的写序最后
        -- 才写入，行级守卫在指针更新时看不到它）；
        -- 跳代、回退与没有后继行一律拒绝（不变量 2、6）。只在原物已取得或收回的 in_custody / committed 放行
        -- （D9 §5.3 撤回 2：旧买方交割已封闭）；in_transit、reconcile 的交割未封闭或未知，不得换代。
        IF OLD.state NOT IN ('in_custody', 'committed')
           OR OLD.current_generation IS NULL
           OR NEW.current_generation IS DISTINCT FROM OLD.current_generation + 1
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_disposition_generations AS successor
               WHERE successor.position_id = NEW.id
                 AND successor.generation = NEW.current_generation
                 AND successor.predecessor = OLD.current_generation
                 AND successor.purpose IN ('relist', 'withdrawal_only')
                 AND successor.xmin = pg_current_xact_id()::xid
           ) THEN
            RAISE EXCEPTION '处分代指针只能前进到同事务写入的唯一后继代（g+1）'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_position_generation_pointer';
        END IF;
    ELSIF OLD.state = 'awaiting_take' AND NEW.state = 'in_custody' THEN
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
    ELSIF OLD.state IN ('in_custody', 'committed', 'in_transit', 'reconcile')
          AND NEW.state = 'season_ended' THEN
        -- 已承诺、在途与核对中的件同样在季窗结束后终结（000137，J8）；退款与交付责任不在此处理。
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
    ELSIF OLD.state = 'committed' AND NEW.state = 'in_custody' THEN
        -- 回库（000137，7C-3）：当前处分代的 Sale 已有封闭事实，且原件从未离库或已被精确收回；同事务先写
        -- uncommitted 事件（引用该封闭事实，版本即本次新版本），处分代不变。原件是否真的在库由
        -- c7_sale_original_back 按派发尝试与退件接管核对（不变量 2、4、6）。
        IF NEW.current_generation IS DISTINCT FROM OLD.current_generation
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_custody_events AS event_row
               JOIN public.consignment_contracts AS contract
                 ON contract.contract_id = event_row.proof_id
               JOIN public.consignment_sale_closures AS closure
                 ON closure.contract_id = contract.contract_id
               WHERE event_row.position_id = NEW.id AND event_row.version = NEW.version
                 AND event_row.event_kind = 'uncommitted'
                 AND event_row.xmin = pg_current_xact_id()::xid
                 AND contract.position_id = NEW.id AND contract.generation = NEW.current_generation
                 AND contract.kind = 'sale'
                 AND public.c7_sale_original_back(contract.contract_id)
           ) THEN
            RAISE EXCEPTION '回库必须同事务引用当前代已封闭的 Sale，且原件从未离库或已被精确收回'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_position_recall_fact';
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
    ELSIF OLD.state IN ('in_custody', 'committed', 'in_transit', 'reconcile')
          AND NEW.state = 'lost' THEN
        -- 已证灭失（000137，J8）：指针留在确认时的当前代（D9 §5.3，不新增处分代），且同事务必须写入
        -- 绑定本 position 与当前代的损失事实；损失事实的外键保证它引用同 position 的损失证据。
        IF NEW.current_generation IS DISTINCT FROM OLD.current_generation
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_loss_facts AS fact
               WHERE fact.position_id = NEW.id AND fact.generation = NEW.current_generation
                 AND fact.xmin = pg_current_xact_id()::xid
           ) THEN
            RAISE EXCEPTION '灭失必须同事务写入当前处分代的损失事实且不替换处分代'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_position_loss_fact';
        END IF;
    ELSE
        RAISE EXCEPTION 'position 迁移不合法'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_position_transition';
    END IF;
    RETURN NEW;
END
$function$;

-- ⑪ 7A-2：退回处置、自动重列与付款提醒（本节全部追加在授权重放之前）。
-- 退回处置（D9 §5.1）：每个精确 Take 消费只一次处置（不变量 1、8）。矩阵的决定逐列保存，供下游写端
-- （退款调用、合同封闭、重列）读取；auto_relist_enabled 与 config_digest 是决策时读一次的冻结值（J12），
-- 之后改配置不影响已作的决定。只追加，更正形成新事实。
CREATE TABLE public.consignment_return_dispositions (
    disposition_id uuid NOT NULL CONSTRAINT c7_disposition_pk PRIMARY KEY,
    consumption_id uuid NOT NULL CONSTRAINT c7_disposition_consumption_uq UNIQUE
        CONSTRAINT c7_disposition_consumption_fk
        REFERENCES public.consignment_return_consumptions(id) ON DELETE RESTRICT,
    contract_id uuid NOT NULL CONSTRAINT c7_disposition_contract_fk
        REFERENCES public.consignment_contracts(contract_id) ON DELETE RESTRICT,
    leg text NOT NULL,
    buyer_right text NOT NULL,
    refund text NOT NULL,
    principal text NOT NULL,
    listing_effect text NOT NULL,
    relist text NOT NULL,
    auto_relist_enabled boolean NOT NULL,
    config_digest bytea NOT NULL,
    command_id uuid NOT NULL CONSTRAINT c7_disposition_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    decided_at timestamptz NOT NULL,
    CONSTRAINT c7_disposition_id_ck CHECK (public.c4_valid_uuid(disposition_id)),
    CONSTRAINT c7_disposition_leg_ck CHECK (leg IN (
        'original', 'carrier', 'cash_refund', 'cash_payout', 'cash_principal')),
    CONSTRAINT c7_disposition_buyer_right_ck CHECK (buyer_right IN (
        'retain', 'close_at_limit', 'terminate_unpaid', 'convert_to_refund',
        'terminate_carrier_leg_only', 'close_old_season', 'not_applicable', 'hold_for_case')),
    CONSTRAINT c7_disposition_refund_ck CHECK (refund IN ('none', 'refund_p', 'restore_refund_purpose')),
    CONSTRAINT c7_disposition_principal_ck CHECK (principal IN (
        'none', 'restore_original_carrier', 'write_off_only')),
    CONSTRAINT c7_disposition_listing_ck CHECK (listing_effect IN (
        'unchanged', 'held_unpaid_recovered', 'held_paid_returned')),
    CONSTRAINT c7_disposition_relist_ck CHECK (relist IN ('forbidden', 'manual_only', 'manual_or_auto')),
    -- 只有把挂单转为下架（原件已收回）的处置才可能再售；其余一律禁止，不变量 6。
    CONSTRAINT c7_disposition_relist_listing_ck CHECK (
        (relist = 'forbidden' OR listing_effect <> 'unchanged') IS TRUE
    ),
    CONSTRAINT c7_disposition_digest_ck CHECK (public.c4_valid_digest(config_digest)),
    CONSTRAINT c7_disposition_time_ck CHECK (isfinite(decided_at))
);
CREATE TRIGGER c7_return_dispositions_append_only
BEFORE UPDATE OR DELETE ON public.consignment_return_dispositions
FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER c7_return_dispositions_no_truncate
BEFORE TRUNCATE ON public.consignment_return_dispositions
FOR EACH STATEMENT EXECUTE FUNCTION public.consignment_reject_fact_mutation();

-- 命令账本闭集：新增 resolve_return / auto_relist / remind 三个 worker 操作与 contract 目标种类。
-- 以下约束与函数都以本文件上文（7M-2）的文本为底，只多本节注明的分支。
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
                'receipt_key', 'buyer_cycle', 'position', 'right', 'browser', 'contract')
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
                    -- 浏览器目标（000137，J5）没有 UUID 目标，只有 request_target 上的 64 位十六进制绑定摘要。
                    WHEN 'browser' THEN target_id IS NULL AND target_bot_uid IS NULL
                        AND target_world_id IS NULL AND target_external_mid IS NULL
                        AND target_season_id IS NULL AND target_calendar_version IS NULL
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
                    'issue_reservation', 'withdraw', 'purchase', 'reprice', 'relist')
                    AND public.consignment_positive_uuid(actor_ref)
                WHEN 'admin' THEN operation IN (
                    'recheck_quarantine', 'release_quarantine', 'admin_delist', 'release_hold',
                    'confirm_position_loss', 'record_loss_case_outcome')
                    AND public.consignment_valid_text(actor_ref, 256) AND strpos(actor_ref, ':') = 0
                WHEN 'worker' THEN operation IN (
                    'expire_reservation', 'decide_envelope', 'append_quarantine_observation',
                    'start_disposition_write', 'apply_mail_proof', 'season_notice', 'season_close',
                    'authorize_dispatch', 'start_attempt', 'mark_write_started', 'record_evidence',
                    'acquire_return', 'start_return_take', 'consume_return',
                    'register_opening_attribution', 'certify_cod', 'prove_handoff', 'settle_cash',
                    'convert_refund', 'open_sanction', 'record_position_loss_proof',
                    'fulfill_withdraw_intent', 'resolve_return', 'auto_relist', 'remind')
                    AND public.consignment_valid_text(actor_ref, 256)
                WHEN 'system' THEN operation IN (
                    'release_quarantine', 'recheck_quarantine', 'acquire_return')
                    AND public.consignment_valid_text(actor_ref, 256)
                -- 受限权利的浏览器与权利人（000137，J5）：actor_ref 分别是 64 位小写十六进制的浏览器绑定摘要
                -- 与 right_id 文本；二者没有登录账号，命令 ID 仍是 UUIDv4（见 consignment_command_uuid_version）。
                WHEN 'browser' THEN operation IN ('locate_right', 'challenge_right', 'verify_right')
                    AND actor_ref COLLATE "C" ~ '^[0-9a-f]{64}$'
                WHEN 'right_holder' THEN operation = 'claim_right'
                    AND public.consignment_positive_uuid(actor_ref)
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
                                      'purchase', 'reprice', 'relist',
                                      'fulfill_withdraw_intent', 'auto_relist'))
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
                    AND operation IN ('start_return_take', 'consume_return', 'resolve_return'))
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
                OR (target_kind = 'position'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation IN ('confirm_position_loss', 'record_loss_case_outcome',
                                      'record_position_loss_proof'))
                OR (target_kind = 'right'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation = 'claim_right')
                OR (target_kind = 'contract'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation = 'remind')
                OR (target_kind = 'browser'
                    AND request_target COLLATE "C" ~ '^[0-9a-f]{64}$' AND listing_id IS NULL
                    AND operation IN ('locate_right', 'challenge_right', 'verify_right'))
            )
        )
    );

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
                'participant_ineligible', 'bot_party', 'action_not_allowed', 'self_purchase',
                'challenge_invalid'))
            OR (status = 404 AND body->>'code' IN ('consignment_disabled', 'listing_not_found'))
            OR (status = 409 AND body->>'code' IN (
                'stale_version', 'listing_not_active', 'equipment_busy', 'equipment_committed',
                'custody_unproven', 'operations_hold', 'season_closed', 'hold_mismatch',
                'source_mismatch', 'evidence_unproven', 'evidence_conflict',
                'disposition_conflict', 'fence_mismatch', 'price_changed', 'funding_boundary',
                'opposite_payment_open', 'lease_lost', 'right_expired'))
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
            -- 直接撤回回执（000137）：Withdrawal 合同已同事务建立，回执带合同与装备、不带意愿。
            IF disposition = 'withdrawal_accepted' THEN
                RETURN status = 202 AND public.c7_valid_withdrawal_accepted(body);
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
        -- ===== 以下为 000137 新增分支（与底本其余部分逐字相同）。COALESCE：缺字段时 STRICT 函数返回 NULL，
        -- 而 CHECK 会放过 NULL，新分支一律失败关闭。
        WHEN 'relist' THEN
            RETURN COALESCE(status = 200 AND disposition = 'relisted'
                AND public.c4_valid_json_uuid(body->'listing_id')
                AND public.c4_valid_json_version(body->'listing_version')
                AND body->>'state' = 'available'
                AND public.c6_valid_json_positive(body->'price')
                AND public.c4_valid_json_version(body->'generation'), false);
        WHEN 'fulfill_withdraw_intent' THEN
            RETURN status = 202 AND disposition = 'withdrawal_accepted'
                AND public.c7_valid_withdrawal_accepted(body);
        WHEN 'confirm_position_loss' THEN
            -- 无合同、退款、赔款或新代字段：损失终局不产生任何资金或处分代（D9 §5.3）。
            RETURN COALESCE(status = 200 AND disposition = 'position_loss_recorded'
                AND public.c4_valid_json_uuid(body->'listing_id')
                AND public.c4_valid_json_version(body->'listing_version')
                AND body->>'state' = 'closed'
                AND public.c4_valid_json_uuid(body->'position_id')
                AND public.c4_valid_json_version(body->'position_version')
                AND public.c4_valid_json_uuid(body->'loss_fact_id')
                AND public.c4_valid_json_uuid(body->'loss_case_id')
                AND public.c4_valid_json_uuid(body->'seller_receipt_id')
                AND NOT body ?| ARRAY['contract_id', 'obligation_id', 'refund_id', 'amount',
                                      'generation'], false);
        WHEN 'record_loss_case_outcome' THEN
            RETURN COALESCE(status = 200 AND disposition = 'case_outcome_recorded'
                AND public.c4_valid_json_uuid(body->'case_id')
                AND public.c4_valid_json_version(body->'case_version')
                AND public.c4_valid_json_uuid(body->'outcome_id')
                AND public.c4_valid_json_uuid(body->'seller_receipt_id')
                AND NOT body ?| ARRAY['amount', 'refund_id', 'payment_id', 'issue_id'], false);
        WHEN 'record_position_loss_proof' THEN
            RETURN COALESCE(status = 200 AND disposition = 'loss_proof_recorded'
                AND public.c4_valid_json_uuid(body->'position_id')
                AND public.c4_valid_json_uuid(body->'proof_id')
                AND public.c4_valid_json_digest(body->'proof_digest'), false);
        WHEN 'locate_right', 'challenge_right' THEN
            RETURN COALESCE(status = 202 AND disposition = 'request_received'
                AND public.c4_valid_json_uuid(body->'challenge_id')
                AND public.c4_valid_json_time(body->'expires_at'), false);
        WHEN 'verify_right' THEN
            RETURN COALESCE(status = 200 AND disposition = 'right_verified'
                AND public.c4_valid_json_uuid(body->'challenge_id')
                AND public.c4_valid_json_uuid(body->'right_id')
                AND public.c4_valid_json_version(body->'right_version')
                AND public.c4_valid_json_time(body->'expires_at'), false);
        WHEN 'claim_right' THEN
            RETURN COALESCE(
                ((status = 202 AND disposition = 'claim_accepted')
                 OR (status = 200 AND disposition IN ('already_fulfilling', 'already_completed')))
                AND public.c4_valid_json_uuid(body->'right_id')
                AND public.c4_valid_json_version(body->'right_version'), false);
        -- ===== 以下三个分支为 7A-2 新增（退回处置、自动重列、付款提醒）；缺字段一律失败关闭。
        WHEN 'resolve_return' THEN
            RETURN COALESCE(status = 200 AND disposition = 'disposition_recorded'
                AND public.c4_valid_json_uuid(body->'contract_id')
                AND public.c4_valid_json_version(body->'contract_version')
                AND public.c4_valid_json_uuid(body->'consumption_id')
                AND public.c4_valid_json_uuid(body->'disposition_id')
                -- 未付收回把挂单转下架时，回执带这一次的挂单变更（与其他改挂单的命令同形）。
                AND (NOT body ? 'listing_changes'
                     OR public.c4_valid_listing_changes(body->'listing_changes')), false);
        WHEN 'auto_relist' THEN
            -- relisted 与手动重列同形（挂单回到 available，带新代序号）；held 不改挂单，只回执当前版本。
            RETURN COALESCE(status = 200 AND (
                (disposition = 'relisted'
                    AND public.c4_valid_json_uuid(body->'listing_id')
                    AND public.c4_valid_json_version(body->'listing_version')
                    AND body->>'state' = 'available'
                    AND public.c6_valid_json_positive(body->'price')
                    AND public.c4_valid_json_version(body->'generation'))
                OR (disposition = 'held'
                    AND public.c4_valid_json_uuid(body->'listing_id')
                    AND public.c4_valid_json_version(body->'listing_version')
                    AND body->>'state' = 'held_off_market'
                    AND NOT body ? 'generation')), false);
        WHEN 'remind' THEN
            RETURN COALESCE(status = 200 AND public.c4_valid_json_uuid(body->'contract_id') AND (
                (disposition = 'reminder_queued'
                    AND public.c4_valid_json_uuid(body->'delivery_id'))
                OR (disposition = 'reminder_voided'
                    AND body->>'decision' IN ('void_paid', 'void_cancelled', 'void_round')
                    AND NOT body ? 'delivery_id')), false);
        ELSE
            RETURN false;
    END CASE;
END
$function$;

-- 自动重列的 relisted 事件绑定 worker 的 auto_relist 命令；其余分支与上文逐字相同。
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
    ) IS TRUE);

-- g+1 重列代的后继命令放行 worker 的 auto_relist；其余核对与上文逐字相同。
CREATE OR REPLACE FUNCTION public.c7_check_generation_successor() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NEW.generation = 1 THEN
        RETURN NULL;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM public.consignment_commands AS command_row
        WHERE command_row.command_id = NEW.created_command_id
          AND command_row.xmin = pg_current_xact_id()::xid
          AND command_row.result_status < 400
          AND CASE NEW.purpose
              WHEN 'relist' THEN
                  (command_row.operation = 'relist' AND command_row.actor_kind = 'public_user')
                  OR (command_row.operation = 'auto_relist' AND command_row.actor_kind = 'worker')
              WHEN 'withdrawal_only' THEN
                  (command_row.operation = 'withdraw' AND command_row.actor_kind = 'public_user')
                  OR (command_row.operation = 'fulfill_withdraw_intent'
                      AND command_row.actor_kind = 'worker')
              ELSE false END
    ) THEN
        RAISE EXCEPTION '处分代后继缺少同事务的成功命令'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_generation_successor_command';
    END IF;
    IF NEW.purpose = 'withdrawal_only' AND NOT EXISTS (
        SELECT 1 FROM public.consignment_contracts AS contract
        JOIN public.consignment_withdrawals AS withdrawal
          ON withdrawal.contract_id = contract.contract_id
        WHERE contract.position_id = NEW.position_id AND contract.generation = NEW.generation
          AND contract.kind = 'withdrawal' AND contract.xmin = pg_current_xact_id()::xid
          AND contract.origin_command_id = NEW.created_command_id
          AND withdrawal.predecessor_generation = NEW.predecessor
    ) THEN
        RAISE EXCEPTION '撤回代缺少同事务的 Withdrawal 合同与前驱代引用'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_generation_successor_contract';
    END IF;
    -- 当前指针 CAS（D9 §5.3 撤回 3）：同事务内 position 的 current_generation 必须恰好前进到本代。
    IF NOT EXISTS (
        SELECT 1 FROM public.consignment_custody_positions AS position_row
        WHERE position_row.id = NEW.position_id
          AND position_row.current_generation = NEW.generation
          AND position_row.xmin = pg_current_xact_id()::xid
    ) THEN
        RAISE EXCEPTION '处分代后继必须同事务把 position 的当前指针推进到本代'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_generation_successor_pointer';
    END IF;
    RETURN NULL;
END
$function$;

-- 挂单迁移（7A-2）：整函数以本文件上文（7M-2）的 consignment_guard_listing 为底，只新增
-- sale_pending -> held_off_market（未付收回）分支；其余分支与上文逐字相同。
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
               OR NEW.reason_code IS NULL
               OR NEW.reason_code NOT IN ('season_closed', 'loss_confirmed')
               OR (NEW.reason_code = 'loss_confirmed'
                   AND NOT public.c7_listing_loss_in_xact(NEW.id)) THEN
                -- 已证灭失（000137）：同事务的损失事实已把 position 推到 lost；事件与命令由事件核对绑定。
                RAISE EXCEPTION '寄售可售态关闭只能因季末或已证灭失，且不能改价、快照或期限'
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
        ELSIF NEW.state = 'available' THEN
            -- 重列（000137）：价格、快照与期限不变且清空下架原因；同事务的重列命令、g+1 重列代与
            -- relisted 事件由 consignment_check_listing_event 逐项绑定（不变量 2、3）。
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at
               OR NEW.reason_code IS NOT NULL THEN
                RAISE EXCEPTION '寄售重列不能改价、快照或期限，且必须清空下架原因'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSIF NEW.state = 'closed' THEN
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at
               OR NEW.reason_code IS NULL
               OR NEW.reason_code NOT IN ('season_closed', 'loss_confirmed')
               OR (NEW.reason_code = 'loss_confirmed'
                   AND NOT public.c7_listing_loss_in_xact(NEW.id)) THEN
                RAISE EXCEPTION '寄售已下架态关闭只能因季末或已证灭失，且不能改价、快照或期限'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSE
            RAISE EXCEPTION '寄售已下架态不能迁移到该状态'
                USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
        END IF;
    ELSIF OLD.state = 'sale_pending' THEN
        -- 成交（000136）：只有买家确领原件（position 同事务进入 handed_over）才关闭为 sold；
        -- 未付收回转下架（7A-2）见下方 held_off_market 分支；止售转移与撤回另有归属。
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
        ELSIF NEW.state = 'held_off_market' THEN
            -- 未付收回（7A-2）：同事务必须已写入本挂单 Sale 的处置行（关闭原买方交割、挂单效果为
            -- held_unpaid_recovered）与 cancelled_unpaid 封闭事实；事件与回执由 consignment_check_listing_event
            -- 绑定 worker 的 resolve_return 命令（不变量 2、3、6）。
            IF NEW.price <> OLD.price
               OR NEW.current_snapshot_id IS DISTINCT FROM OLD.current_snapshot_id
               OR NEW.expires_at <> OLD.expires_at
               OR NEW.reason_code IS DISTINCT FROM 'unpaid_recovered'
               OR NOT EXISTS (
                   SELECT 1 FROM public.consignment_return_dispositions AS disposition
                   JOIN public.consignment_contracts AS contract
                     ON contract.contract_id = disposition.contract_id
                   JOIN public.consignment_sale_closures AS closure
                     ON closure.contract_id = contract.contract_id
                   WHERE contract.listing_id = NEW.id AND contract.kind = 'sale'
                     AND disposition.listing_effect = 'held_unpaid_recovered'
                     AND disposition.buyer_right IN ('close_at_limit', 'terminate_unpaid')
                     AND closure.closure_kind = 'cancelled_unpaid'
                     AND disposition.xmin = pg_current_xact_id()::xid
                     AND closure.xmin = pg_current_xact_id()::xid
               ) THEN
                RAISE EXCEPTION '寄售未付收回下架必须同事务写入处置行与未付取消封闭，且不能改价、快照或期限'
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
               OR NEW.reason_code IS NULL
               OR NEW.reason_code NOT IN ('season_closed', 'withdrawn_delivered')
               OR (NEW.reason_code = 'withdrawn_delivered'
                   AND NOT public.c7_listing_withdrawn_in_xact(NEW.id)) THEN
                -- 已证原卖家领取（000137）：同事务必须写入 consignment_withdrawn 交接边。
                RAISE EXCEPTION '寄售撤回态关闭只能因季末或已证原卖家领取，且不能改价、快照或期限'
                    USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
            END IF;
        ELSE
            RAISE EXCEPTION '寄售撤回态不能迁移到该状态'
                USING ERRCODE = '23514', CONSTRAINT = 'consignment_listing_transition';
        END IF;
    END IF;

    -- 000137 起 withdrawn_delivered 与 loss_confirmed 由上面各自的分支放行并绑定同事务事实，不再有恒拒尾段。
    RETURN NEW;
END
$function$;

-- ⑫ 7B-2：处罚执行与解禁周期重置的命令账本操作，以及“同周期已被他途拉黑”的效果状态（本节追加在授权重放之前）。
-- 命令账本闭集：新增 apply_sanction（worker，目标种类 sanction，回执 sanction_applied：sanction_id、effect_state
-- （applied / cycle_closed / already_blacklisted）、applied 时必填的 account_command_id）与 reset_cycle（system，目标种类
-- buyer_cycle，目标是买家；回执 cycle_opened：buyer_user_id、closed_cycle_id、opened_cycle_id、可空 closed_sanction_id）。
-- 以下约束与函数以本文件上文（7A-2 节）的文本为底，只多本节注明的取值与分支（逐函数核对过，未丢任何既有取值）。
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
                'receipt_key', 'buyer_cycle', 'position', 'right', 'browser', 'contract',
                'sanction')
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
                    -- 浏览器目标（000137，J5）没有 UUID 目标，只有 request_target 上的 64 位十六进制绑定摘要。
                    WHEN 'browser' THEN target_id IS NULL AND target_bot_uid IS NULL
                        AND target_world_id IS NULL AND target_external_mid IS NULL
                        AND target_season_id IS NULL AND target_calendar_version IS NULL
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
                    'issue_reservation', 'withdraw', 'purchase', 'reprice', 'relist')
                    AND public.consignment_positive_uuid(actor_ref)
                WHEN 'admin' THEN operation IN (
                    'recheck_quarantine', 'release_quarantine', 'admin_delist', 'release_hold',
                    'confirm_position_loss', 'record_loss_case_outcome')
                    AND public.consignment_valid_text(actor_ref, 256) AND strpos(actor_ref, ':') = 0
                WHEN 'worker' THEN operation IN (
                    'expire_reservation', 'decide_envelope', 'append_quarantine_observation',
                    'start_disposition_write', 'apply_mail_proof', 'season_notice', 'season_close',
                    'authorize_dispatch', 'start_attempt', 'mark_write_started', 'record_evidence',
                    'acquire_return', 'start_return_take', 'consume_return',
                    'register_opening_attribution', 'certify_cod', 'prove_handoff', 'settle_cash',
                    'convert_refund', 'open_sanction', 'record_position_loss_proof',
                    'fulfill_withdraw_intent', 'resolve_return', 'auto_relist', 'remind',
                    'apply_sanction')
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
                                      'purchase', 'reprice', 'relist',
                                      'fulfill_withdraw_intent', 'auto_relist'))
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
                    AND operation IN ('start_return_take', 'consume_return', 'resolve_return'))
                OR (target_kind = 'receipt_key'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation = 'register_opening_attribution')
                OR (target_kind = 'obligation'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation IN ('certify_cod', 'prove_handoff', 'settle_cash',
                                      'convert_refund'))
                OR (target_kind = 'buyer_cycle'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation IN ('open_sanction', 'reset_cycle'))
                OR (target_kind = 'position'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation IN ('confirm_position_loss', 'record_loss_case_outcome',
                                      'record_position_loss_proof'))
                OR (target_kind = 'right'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation = 'claim_right')
                OR (target_kind = 'contract'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation = 'remind')
                OR (target_kind = 'sanction'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation = 'apply_sanction')
                OR (target_kind = 'browser'
                    AND request_target COLLATE "C" ~ '^[0-9a-f]{64}$' AND listing_id IS NULL
                    AND operation IN ('locate_right', 'challenge_right', 'verify_right'))
            )
        )
    );


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
                'participant_ineligible', 'bot_party', 'action_not_allowed', 'self_purchase',
                'challenge_invalid'))
            OR (status = 404 AND body->>'code' IN ('consignment_disabled', 'listing_not_found'))
            OR (status = 409 AND body->>'code' IN (
                'stale_version', 'listing_not_active', 'equipment_busy', 'equipment_committed',
                'custody_unproven', 'operations_hold', 'season_closed', 'hold_mismatch',
                'source_mismatch', 'evidence_unproven', 'evidence_conflict',
                'disposition_conflict', 'fence_mismatch', 'price_changed', 'funding_boundary',
                'opposite_payment_open', 'lease_lost', 'right_expired'))
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
            -- 直接撤回回执（000137）：Withdrawal 合同已同事务建立，回执带合同与装备、不带意愿。
            IF disposition = 'withdrawal_accepted' THEN
                RETURN status = 202 AND public.c7_valid_withdrawal_accepted(body);
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
        -- ===== 以下为 000137 新增分支（与底本其余部分逐字相同）。COALESCE：缺字段时 STRICT 函数返回 NULL，
        -- 而 CHECK 会放过 NULL，新分支一律失败关闭。
        WHEN 'relist' THEN
            RETURN COALESCE(status = 200 AND disposition = 'relisted'
                AND public.c4_valid_json_uuid(body->'listing_id')
                AND public.c4_valid_json_version(body->'listing_version')
                AND body->>'state' = 'available'
                AND public.c6_valid_json_positive(body->'price')
                AND public.c4_valid_json_version(body->'generation'), false);
        WHEN 'fulfill_withdraw_intent' THEN
            RETURN status = 202 AND disposition = 'withdrawal_accepted'
                AND public.c7_valid_withdrawal_accepted(body);
        WHEN 'confirm_position_loss' THEN
            -- 无合同、退款、赔款或新代字段：损失终局不产生任何资金或处分代（D9 §5.3）。
            RETURN COALESCE(status = 200 AND disposition = 'position_loss_recorded'
                AND public.c4_valid_json_uuid(body->'listing_id')
                AND public.c4_valid_json_version(body->'listing_version')
                AND body->>'state' = 'closed'
                AND public.c4_valid_json_uuid(body->'position_id')
                AND public.c4_valid_json_version(body->'position_version')
                AND public.c4_valid_json_uuid(body->'loss_fact_id')
                AND public.c4_valid_json_uuid(body->'loss_case_id')
                AND public.c4_valid_json_uuid(body->'seller_receipt_id')
                AND NOT body ?| ARRAY['contract_id', 'obligation_id', 'refund_id', 'amount',
                                      'generation'], false);
        WHEN 'record_loss_case_outcome' THEN
            RETURN COALESCE(status = 200 AND disposition = 'case_outcome_recorded'
                AND public.c4_valid_json_uuid(body->'case_id')
                AND public.c4_valid_json_version(body->'case_version')
                AND public.c4_valid_json_uuid(body->'outcome_id')
                AND public.c4_valid_json_uuid(body->'seller_receipt_id')
                AND NOT body ?| ARRAY['amount', 'refund_id', 'payment_id', 'issue_id'], false);
        WHEN 'record_position_loss_proof' THEN
            RETURN COALESCE(status = 200 AND disposition = 'loss_proof_recorded'
                AND public.c4_valid_json_uuid(body->'position_id')
                AND public.c4_valid_json_uuid(body->'proof_id')
                AND public.c4_valid_json_digest(body->'proof_digest'), false);
        WHEN 'locate_right', 'challenge_right' THEN
            RETURN COALESCE(status = 202 AND disposition = 'request_received'
                AND public.c4_valid_json_uuid(body->'challenge_id')
                AND public.c4_valid_json_time(body->'expires_at'), false);
        WHEN 'verify_right' THEN
            RETURN COALESCE(status = 200 AND disposition = 'right_verified'
                AND public.c4_valid_json_uuid(body->'challenge_id')
                AND public.c4_valid_json_uuid(body->'right_id')
                AND public.c4_valid_json_version(body->'right_version')
                AND public.c4_valid_json_time(body->'expires_at'), false);
        WHEN 'claim_right' THEN
            RETURN COALESCE(
                ((status = 202 AND disposition = 'claim_accepted')
                 OR (status = 200 AND disposition IN ('already_fulfilling', 'already_completed')))
                AND public.c4_valid_json_uuid(body->'right_id')
                AND public.c4_valid_json_version(body->'right_version'), false);
        -- ===== 以下三个分支为 7A-2 新增（退回处置、自动重列、付款提醒）；缺字段一律失败关闭。
        WHEN 'resolve_return' THEN
            RETURN COALESCE(status = 200 AND disposition = 'disposition_recorded'
                AND public.c4_valid_json_uuid(body->'contract_id')
                AND public.c4_valid_json_version(body->'contract_version')
                AND public.c4_valid_json_uuid(body->'consumption_id')
                AND public.c4_valid_json_uuid(body->'disposition_id')
                -- 未付收回把挂单转下架时，回执带这一次的挂单变更（与其他改挂单的命令同形）。
                AND (NOT body ? 'listing_changes'
                     OR public.c4_valid_listing_changes(body->'listing_changes')), false);
        WHEN 'auto_relist' THEN
            -- relisted 与手动重列同形（挂单回到 available，带新代序号）；held 不改挂单，只回执当前版本。
            RETURN COALESCE(status = 200 AND (
                (disposition = 'relisted'
                    AND public.c4_valid_json_uuid(body->'listing_id')
                    AND public.c4_valid_json_version(body->'listing_version')
                    AND body->>'state' = 'available'
                    AND public.c6_valid_json_positive(body->'price')
                    AND public.c4_valid_json_version(body->'generation'))
                OR (disposition = 'held'
                    AND public.c4_valid_json_uuid(body->'listing_id')
                    AND public.c4_valid_json_version(body->'listing_version')
                    AND body->>'state' = 'held_off_market'
                    AND NOT body ? 'generation')), false);
        WHEN 'remind' THEN
            RETURN COALESCE(status = 200 AND public.c4_valid_json_uuid(body->'contract_id') AND (
                (disposition = 'reminder_queued'
                    AND public.c4_valid_json_uuid(body->'delivery_id'))
                OR (disposition = 'reminder_voided'
                    AND body->>'decision' IN ('void_paid', 'void_cancelled', 'void_round')
                    AND NOT body ? 'delivery_id')), false);
        -- ===== 以下两个分支为 7B-2 新增（处罚执行、解禁周期重置）；缺字段一律失败关闭。
        -- apply_sanction：回执带处罚与效果状态；applied 必带账号拉黑命令，其余状态不带（已禁仅记录不引用命令）。
        WHEN 'apply_sanction' THEN
            RETURN COALESCE(status = 200 AND disposition = 'sanction_applied'
                AND public.c4_valid_json_uuid(body->'sanction_id')
                AND body->>'effect_state' IN ('applied', 'cycle_closed', 'already_blacklisted')
                AND ((body->>'effect_state' = 'applied')
                     = COALESCE(public.c4_valid_json_uuid(body->'account_command_id'), false)), false);
        -- reset_cycle：closed_cycle_id / closed_sanction_id 键必须在，值为 UUID 或 null；有关闭的处罚必有关闭的周期。
        WHEN 'reset_cycle' THEN
            RETURN COALESCE(status = 200 AND disposition = 'cycle_opened'
                AND public.c4_valid_json_uuid(body->'buyer_user_id')
                AND public.c4_valid_json_uuid(body->'opened_cycle_id')
                AND (jsonb_typeof(body->'closed_cycle_id') = 'null'
                     OR public.c4_valid_json_uuid(body->'closed_cycle_id'))
                AND (jsonb_typeof(body->'closed_sanction_id') = 'null'
                     OR (public.c4_valid_json_uuid(body->'closed_sanction_id')
                         AND jsonb_typeof(body->'closed_cycle_id') = 'string')), false);
        ELSE
            RETURN false;
    END CASE;
END
$function$;

-- 处罚效果新增状态 already_blacklisted：账号因别的途径已被禁，只记录、不引用任何拉黑命令（统筹方 11-24 裁定，
-- 取代“引用买家名下最新拉黑命令”）。账号命令列的约束 c7_effect_command_ck（applied 当且仅当带命令）原样成立：
-- 该状态命令恒为空。处罚投影记 applied（新购阻断随 pending 解除，账号状态本身继续阻断），账号工作记 completed，
-- 且触发器核对账号当前确为 blacklisted。
ALTER TABLE public.consignment_sanction_effects
    DROP CONSTRAINT c7_effect_state_ck;
ALTER TABLE public.consignment_sanction_effects
    ADD CONSTRAINT c7_effect_state_ck CHECK (state IN ('applied', 'cycle_closed', 'already_blacklisted'));
ALTER TABLE public.consignment_sanction_work_results
    DROP CONSTRAINT c7_work_result_ck;
ALTER TABLE public.consignment_sanction_work_results
    ADD CONSTRAINT c7_work_result_ck CHECK (result IN ('applied', 'cycle_closed', 'already_blacklisted'));
CREATE OR REPLACE FUNCTION public.c7_check_sanction_effect() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF TG_TABLE_NAME = 'consignment_sanction_effects' THEN
        IF NOT EXISTS (
            SELECT 1 FROM public.consignment_sanctions AS sanction
            WHERE sanction.id = NEW.sanction_id
              AND sanction.state = CASE NEW.state WHEN 'already_blacklisted' THEN 'applied'
                                                  ELSE NEW.state END
        ) OR (NEW.state = 'applied' AND NOT EXISTS (
            SELECT 1 FROM public.user_admin_commands AS account_command
            WHERE account_command.command_id = NEW.account_command_id
              AND account_command.command_kind = 'blacklist'
              AND account_command.user_id = NEW.buyer_user_id
        )) OR (NEW.state = 'already_blacklisted' AND NOT EXISTS (
            SELECT 1 FROM public.users AS account
            WHERE account.id = NEW.buyer_user_id AND account.status = 'blacklisted'
        )) THEN
            RAISE EXCEPTION '处罚效果必须与处罚状态一致，applied 必带该买家的拉黑命令，already_blacklisted 须账号确已被禁'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_effect_projection';
        END IF;
    ELSIF NOT EXISTS (
        SELECT 1 FROM public.consignment_account_work AS work
        WHERE work.id = NEW.work_id AND work.sanction_id = NEW.sanction_id
          AND work.state = CASE NEW.result WHEN 'cycle_closed' THEN 'cancelled' ELSE 'completed' END
    ) OR NOT EXISTS (
        SELECT 1 FROM public.consignment_sanction_effects AS effect
        WHERE effect.sanction_id = NEW.sanction_id AND effect.state = NEW.result
    ) THEN
        RAISE EXCEPTION '账号工作结果必须与处罚效果和工作状态一致'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_work_result_projection';
    END IF;
    RETURN NULL;
END
$function$;

-- ⑬ 7C-4b：灭失“无法查清”的核对记录（本节追加在授权重放之前）。
-- D9 §5.3 第三行要求“保留原物占用、买卖双方责任和核对案”：确认命令因当前代权责或相反外写读不清而被拒
-- （确定拒绝 evidence_conflict）时，同事务追加一条按 position 持久的核对记录；它不迁移任何状态，也不决定金额、
-- 资格或占用（不变量 4、6、8）。同一 position 每次被拒各有一条（命令 ID 主键）；运维告警信号只作提醒。
CREATE TABLE public.consignment_loss_reconciles (
    command_id uuid NOT NULL CONSTRAINT c7_reconcile_pk PRIMARY KEY
        CONSTRAINT c7_reconcile_command_fk REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    position_id uuid NOT NULL CONSTRAINT c7_reconcile_position_fk
        REFERENCES public.consignment_custody_positions(id) ON DELETE RESTRICT,
    recorded_at timestamptz NOT NULL,
    CONSTRAINT c7_reconcile_time_ck CHECK (isfinite(recorded_at))
);
CREATE INDEX c7_reconcile_position_idx
    ON public.consignment_loss_reconciles (position_id, recorded_at);

-- 核对记录必须对应同一 position 上被拒为 evidence_conflict 的灭失确认命令（COMMIT 前核对，命令行最后才写入）。
CREATE FUNCTION public.c7_check_loss_reconcile() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM public.consignment_commands AS command_row
        WHERE command_row.command_id = NEW.command_id
          AND command_row.operation = 'confirm_position_loss'
          AND command_row.target_kind = 'position' AND command_row.target_id = NEW.position_id
          AND command_row.result_status = 409 AND command_row.result_code = 'evidence_conflict'
    ) THEN
        RAISE EXCEPTION '灭失核对记录必须对应同一 position 上被拒为 evidence_conflict 的确认命令'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_reconcile_command';
    END IF;
    RETURN NULL;
END
$function$;
CREATE CONSTRAINT TRIGGER c7_reconcile_command_binding
AFTER INSERT ON public.consignment_loss_reconciles
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.c7_check_loss_reconcile();
CREATE TRIGGER c7_consignment_loss_reconciles_append_only
BEFORE UPDATE OR DELETE ON public.consignment_loss_reconciles
FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER c7_consignment_loss_reconciles_no_truncate
BEFORE TRUNCATE ON public.consignment_loss_reconciles
FOR EACH STATEMENT EXECUTE FUNCTION public.consignment_reject_fact_mutation();

-- ======================================================================================================
-- ⑯ 7E-2：Admin 订单 recheck / resolve（D9 §6.2 “Admin recheck/resolve”）
-- 追加在授权重放之前：账本新增 admin 操作 recheck / resolve 与目标种类 contract；两张只追加的新表承载
-- 复核工作与处置记录；回执形状经包装函数校验（7M-2 的整函数文本不动：改名为 c7_valid_v2_receipt_base，
-- 新函数只接管这两个操作的成功分支，其余与拒绝一律委托底本）。CHECK 绑定函数 OID，所以重建回执约束。
-- 处置表的动作 CHECK 取 D9 的完整闭集：cancel_unpaid 经 7A-2 的封闭写端（consignment_sale_closures），
-- retain_entitlement 与退款转换同属 6B 写端（统筹方裁定）；各动作的数据库前置由 c7_guard_order_resolution 核对。
-- ======================================================================================================

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
                'receipt_key', 'buyer_cycle', 'position', 'right', 'browser', 'contract',
                'sanction')
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
                    -- 浏览器目标（000137，J5）没有 UUID 目标，只有 request_target 上的 64 位十六进制绑定摘要。
                    WHEN 'browser' THEN target_id IS NULL AND target_bot_uid IS NULL
                        AND target_world_id IS NULL AND target_external_mid IS NULL
                        AND target_season_id IS NULL AND target_calendar_version IS NULL
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
                    'issue_reservation', 'withdraw', 'purchase', 'reprice', 'relist')
                    AND public.consignment_positive_uuid(actor_ref)
                WHEN 'admin' THEN operation IN (
                    'recheck_quarantine', 'release_quarantine', 'admin_delist', 'release_hold',
                    'confirm_position_loss', 'record_loss_case_outcome', 'recheck', 'resolve')
                    AND public.consignment_valid_text(actor_ref, 256) AND strpos(actor_ref, ':') = 0
                WHEN 'worker' THEN operation IN (
                    'expire_reservation', 'decide_envelope', 'append_quarantine_observation',
                    'start_disposition_write', 'apply_mail_proof', 'season_notice', 'season_close',
                    'authorize_dispatch', 'start_attempt', 'mark_write_started', 'record_evidence',
                    'acquire_return', 'start_return_take', 'consume_return',
                    'register_opening_attribution', 'certify_cod', 'prove_handoff', 'settle_cash',
                    'convert_refund', 'open_sanction', 'record_position_loss_proof',
                    'fulfill_withdraw_intent', 'resolve_return', 'auto_relist', 'remind',
                    'apply_sanction')
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
                                      'purchase', 'reprice', 'relist',
                                      'fulfill_withdraw_intent', 'auto_relist'))
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
                    AND operation IN ('start_return_take', 'consume_return', 'resolve_return'))
                OR (target_kind = 'receipt_key'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation = 'register_opening_attribution')
                OR (target_kind = 'obligation'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation IN ('certify_cod', 'prove_handoff', 'settle_cash',
                                      'convert_refund'))
                OR (target_kind = 'buyer_cycle'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation IN ('open_sanction', 'reset_cycle'))
                OR (target_kind = 'position'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation IN ('confirm_position_loss', 'record_loss_case_outcome',
                                      'record_position_loss_proof'))
                OR (target_kind = 'right'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation = 'claim_right')
                OR (target_kind = 'contract'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation IN ('remind', 'recheck', 'resolve'))
                OR (target_kind = 'sanction'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation = 'apply_sanction')
                OR (target_kind = 'browser'
                    AND request_target COLLATE "C" ~ '^[0-9a-f]{64}$' AND listing_id IS NULL
                    AND operation IN ('locate_right', 'challenge_right', 'verify_right'))
            )
        )
    );

CREATE TABLE public.consignment_order_rechecks (
    work_id uuid NOT NULL CONSTRAINT c7_recheck_pk PRIMARY KEY,
    contract_id uuid NOT NULL CONSTRAINT c7_recheck_contract_fk
        REFERENCES public.consignment_contracts(contract_id) ON DELETE RESTRICT,
    contract_version bigint NOT NULL,
    requested_by text NOT NULL,
    command_id uuid NOT NULL CONSTRAINT c7_recheck_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    requested_at timestamptz NOT NULL,
    CONSTRAINT c7_recheck_id_ck CHECK (public.c4_valid_uuid(work_id)),
    CONSTRAINT c7_recheck_version_ck CHECK (contract_version BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c7_recheck_actor_ck CHECK (public.consignment_valid_text(requested_by, 256)),
    CONSTRAINT c7_recheck_time_ck CHECK (isfinite(requested_at)),
    CONSTRAINT c7_recheck_command_uq UNIQUE (command_id)
);
CREATE INDEX c7_recheck_contract_idx
    ON public.consignment_order_rechecks (contract_id, requested_at, work_id);

CREATE TABLE public.consignment_order_resolutions (
    resolution_id uuid NOT NULL CONSTRAINT c7_resolution_pk PRIMARY KEY,
    contract_id uuid NOT NULL CONSTRAINT c7_resolution_contract_fk
        REFERENCES public.consignment_contracts(contract_id) ON DELETE RESTRICT,
    action text NOT NULL,
    evidence_id uuid NOT NULL,
    -- 仅 redispatch_original：将要开的新轮的轮 ID（dispatch_round_id），Worker 以它授权新轮。
    dispatch_id uuid,
    contract_version bigint NOT NULL,
    resolved_by text NOT NULL,
    command_id uuid NOT NULL CONSTRAINT c7_resolution_command_fk
        REFERENCES public.consignment_commands(command_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    resolved_at timestamptz NOT NULL,
    CONSTRAINT c7_resolution_id_ck CHECK (public.c4_valid_uuid(resolution_id)),
    CONSTRAINT c7_resolution_action_ck CHECK (action IN (
        'cancel_unpaid', 'refund_returned', 'refund_lost', 'refund_season',
        'retain_entitlement', 'redispatch_original')),
    CONSTRAINT c7_resolution_evidence_ck CHECK (public.c4_valid_uuid(evidence_id)),
    CONSTRAINT c7_resolution_dispatch_ck CHECK (
        ((action = 'redispatch_original') = (dispatch_id IS NOT NULL)) IS TRUE
        AND (dispatch_id IS NULL OR public.c4_valid_uuid(dispatch_id))
    ),
    CONSTRAINT c7_resolution_version_ck CHECK (contract_version BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c7_resolution_actor_ck CHECK (public.consignment_valid_text(resolved_by, 256)),
    CONSTRAINT c7_resolution_time_ck CHECK (isfinite(resolved_at)),
    CONSTRAINT c7_resolution_command_uq UNIQUE (command_id),
    -- 同一轮至多排队一次补寄（不变量 4：不无依据重发）。
    CONSTRAINT c7_resolution_dispatch_uq UNIQUE (dispatch_id)
);
-- 退款类处置每个合同至多一条：退款由 6B 单写者至多转换一次，处置记录与之一一对应（不变量 1、4）。
CREATE UNIQUE INDEX c7_resolution_refund_uq
    ON public.consignment_order_resolutions (contract_id)
    WHERE action IN ('refund_returned', 'refund_lost', 'refund_season');
-- 未付取消每个合同至多一条（封闭事实本身唯一）；权利保留每个退回处置至多一条（同一笔回补金额只留存一次）。
CREATE UNIQUE INDEX c7_resolution_cancel_uq
    ON public.consignment_order_resolutions (contract_id)
    WHERE action = 'cancel_unpaid';
CREATE UNIQUE INDEX c7_resolution_retain_uq
    ON public.consignment_order_resolutions (evidence_id)
    WHERE action = 'retain_entitlement';

-- 退款类处置必须以已转换的买家退款为前提；未付取消必须已有同一处置的 cancelled_unpaid 封闭事实；权利保留必须引用
-- 已回补现金的现金腿退回处置；补寄必须引用已消费（明确封闭）的上一轮原件退件接管，且尚无更新的轮
-- （不变量 4、6：数据库是应用层核对之外的最后一道门）。
CREATE FUNCTION public.c7_guard_order_resolution() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NEW.action IN ('refund_returned', 'refund_lost', 'refund_season') THEN
        IF NOT EXISTS (
            SELECT 1 FROM public.consignment_funded_sales AS funded
            WHERE funded.contract_id = NEW.contract_id AND funded.disposition = 'refund_converted'
        ) THEN
            RAISE EXCEPTION '退款类处置要求该合同已转换为买家退款'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_resolution_refund_converted';
        END IF;
    ELSIF NEW.action = 'cancel_unpaid' THEN
        IF NOT EXISTS (
            SELECT 1 FROM public.consignment_sale_closures AS closure
            WHERE closure.contract_id = NEW.contract_id AND closure.closure_kind = 'cancelled_unpaid'
              AND closure.basis_ref = 'unpaid-recovery:' || NEW.evidence_id::text
        ) THEN
            RAISE EXCEPTION '未付取消要求该合同已有同一处置的取消封闭事实'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_resolution_cancel_closed';
        END IF;
    ELSIF NEW.action = 'retain_entitlement' THEN
        IF NOT EXISTS (
            SELECT 1 FROM public.consignment_return_dispositions AS disposition
            JOIN public.consignment_funding_returns AS funding_return
              ON funding_return.consumption_id = disposition.consumption_id
             AND funding_return.contract_id = disposition.contract_id
            WHERE disposition.disposition_id = NEW.evidence_id
              AND disposition.contract_id = NEW.contract_id
              AND disposition.leg IN ('cash_refund', 'cash_payout')
        ) THEN
            RAISE EXCEPTION '权利保留要求现金腿退回处置已有资金回补'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_resolution_retain_returned';
        END IF;
    ELSIF NEW.action = 'redispatch_original' THEN
        IF NOT EXISTS (
            SELECT 1 FROM public.consignment_return_consumptions AS consumption
            WHERE consumption.intake_id = NEW.evidence_id
              AND consumption.contract_id = NEW.contract_id
              AND consumption.leg = 'original'
              AND NOT EXISTS (
                  SELECT 1 FROM public.consignment_dispatch_rounds AS newer
                  WHERE newer.contract_id = consumption.contract_id
                    AND newer.obligation_id = consumption.obligation_id
                    AND newer.round > consumption.original_round)
        ) THEN
            RAISE EXCEPTION '补寄要求上一轮原件退件已消费封闭且尚无更新的轮'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_resolution_redispatch_closed';
        END IF;
    END IF;
    RETURN NEW;
END
$function$;
-- AFTER 触发器：行形状 CHECK 先于数据库前置核对，形状错误报形状约束。
CREATE TRIGGER c7_order_resolution_binding
AFTER INSERT ON public.consignment_order_resolutions
FOR EACH ROW EXECUTE FUNCTION public.c7_guard_order_resolution();

CREATE TRIGGER c7_consignment_order_rechecks_append_only
BEFORE UPDATE OR DELETE ON public.consignment_order_rechecks
FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER c7_consignment_order_rechecks_no_truncate
BEFORE TRUNCATE ON public.consignment_order_rechecks
FOR EACH STATEMENT EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER c7_consignment_order_resolutions_append_only
BEFORE UPDATE OR DELETE ON public.consignment_order_resolutions
FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER c7_consignment_order_resolutions_no_truncate
BEFORE TRUNCATE ON public.consignment_order_resolutions
FOR EACH STATEMENT EXECUTE FUNCTION public.consignment_reject_fact_mutation();

-- 回执形状：recheck 为 202 recheck_queued（带 work_id）；resolve 为 200 resolution_recorded（带
-- resolution_id，动作不是补寄）或 202 redispatch_queued（带 dispatch_id，动作固定补寄）。缺字段一律失败关闭。
ALTER FUNCTION public.c4_valid_v2_receipt(jsonb, uuid, text, integer)
    RENAME TO c7_valid_v2_receipt_base;

CREATE FUNCTION public.c4_valid_v2_receipt(
    body jsonb, command_id uuid, operation text, status integer
) RETURNS boolean
LANGUAGE plpgsql STABLE PARALLEL SAFE SET search_path = pg_catalog, public
AS $function$
BEGIN
    IF status >= 400 OR operation NOT IN ('recheck', 'resolve') THEN
        RETURN public.c7_valid_v2_receipt_base(body, command_id, operation, status);
    END IF;
    IF jsonb_typeof(body) <> 'object'
       OR body->'schema_version' IS DISTINCT FROM '2'::jsonb
       OR body->'command_id' IS DISTINCT FROM to_jsonb(command_id)
       OR body->>'operation' IS DISTINCT FROM operation
       OR jsonb_typeof(body->'disposition') <> 'string' THEN
        RETURN false;
    END IF;
    IF NOT COALESCE(public.c4_valid_json_uuid(body->'contract_id')
        AND public.c4_valid_json_version(body->'contract_version'), false) THEN
        RETURN false;
    END IF;
    IF operation = 'recheck' THEN
        RETURN COALESCE(status = 202 AND body->>'disposition' = 'recheck_queued'
            AND public.c4_valid_json_uuid(body->'work_id'), false);
    END IF;
    RETURN COALESCE(
        (status = 200 AND body->>'disposition' = 'resolution_recorded'
            AND body->>'action' IN ('cancel_unpaid', 'refund_returned', 'refund_lost',
                                    'refund_season', 'retain_entitlement')
            AND public.c4_valid_json_uuid(body->'resolution_id'))
        OR (status = 202 AND body->>'disposition' = 'redispatch_queued'
            AND body->>'action' = 'redispatch_original'
            AND public.c4_valid_json_uuid(body->'dispatch_id')), false);
END
$function$;

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


-- ⑰ 6D6：季末关闭取得工作的投递处置 `season_closed`（维护者 11-24 批准方案 A；本节追加在授权重放之前）。
-- 季末关闭从未起写的 Take 工作时，同事务把其待发投递置为 failed/season_ended（围栏 +1）。该终态从未外写、也不再可执行，
-- 以统一处置事实收口：种类 `season_closed` 自证（以投递行与同事务的工作关闭记录为证，不带外部来源引用），写入时固定
-- `resolves_alert = true`。只放宽处置表的种类闭集与配套 CHECK，不改写既有行；处置表仍只追加（000088 触发器不变）。
-- 下面的插入守卫把该种类限定在“季末关闭、且从未起写”的寄售取得投递上（不变量 4、6、8），别的投递写这一种类一律拒绝。
LOCK TABLE public.delivery_dispositions IN SHARE ROW EXCLUSIVE MODE;

ALTER TABLE public.delivery_dispositions
    DROP CONSTRAINT delivery_dispositions_kind_check,
    DROP CONSTRAINT delivery_dispositions_check1,
    DROP CONSTRAINT delivery_dispositions_check2,
    DROP CONSTRAINT delivery_dispositions_check7,
    ADD CONSTRAINT delivery_dispositions_kind_check CHECK (
        kind IN (
            'auto_sent_proof', 'operator_sent_confirmation', 'admin_failed',
            'equipment_loss', 'cod_returned', 'recovery_equipment_recovered', 'legacy_self',
            'preflight_retired', 'operator_acknowledged', 'season_closed'
        )
    ),
    ADD CONSTRAINT delivery_dispositions_check1 CHECK (
        num_nonnulls(sent_confirmation_id, admin_command_id, equipment_loss_id,
                     legacy_self_reconciliation_id, sent_binding_world_id,
                     settlement_recovery_id, source_audit_id)
        = CASE WHEN kind IN ('preflight_retired', 'season_closed') THEN 0 ELSE 1 END
    ),
    ADD CONSTRAINT delivery_dispositions_check2 CHECK (
        (kind = 'auto_sent_proof' AND sent_binding_world_id IS NOT NULL)
        OR (kind = 'operator_sent_confirmation' AND sent_confirmation_id IS NOT NULL)
        OR (kind = 'admin_failed' AND admin_command_id IS NOT NULL)
        OR (kind = 'equipment_loss' AND equipment_loss_id IS NOT NULL)
        OR (kind = 'cod_returned' AND settlement_recovery_id IS NOT NULL)
        OR (kind = 'recovery_equipment_recovered' AND settlement_recovery_id IS NOT NULL)
        OR (kind = 'legacy_self' AND legacy_self_reconciliation_id IS NOT NULL)
        OR (kind = 'operator_acknowledged' AND source_audit_id IS NOT NULL)
        OR kind IN ('preflight_retired', 'season_closed')
    ),
    ADD CONSTRAINT delivery_dispositions_check7 CHECK (
        (kind IN ('auto_sent_proof', 'operator_sent_confirmation', 'admin_failed',
                  'equipment_loss', 'cod_returned', 'recovery_equipment_recovered',
                  'legacy_self', 'operator_acknowledged', 'season_closed') AND resolves_alert)
        OR (kind = 'preflight_retired'
            AND resolves_alert = (reason_code = 'lot_watch_invalidated'))
    );

-- `season_closed` 只能落在这样的投递上：它是某个寄售 Take 工作的投递，该工作已有季末关闭记录且从未起写；投递本身是
-- failed/season_ended、从未进入发送围栏，处置作用在投递当前 fence 上。关闭记录与投递取消在同一事务先于处置写入。
CREATE FUNCTION public.c7_check_season_closed_disposition() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM public.mail_disposition_work AS work
        JOIN public.consignment_work_closures AS closure ON closure.work_id = work.id
        JOIN public.deliveries AS delivery ON delivery.id = work.delivery_id
        WHERE work.delivery_id = NEW.delivery_id
          AND work.action = 'take'
          AND closure.reason = 'season_ended'
          AND delivery.origin_kind = 'consignment_disposition'
          AND delivery.status = 'failed'
          AND delivery.failure_code = 'season_ended'
          AND delivery.sending_at IS NULL
          AND delivery.fencing_version = NEW.fencing_version
          AND NOT EXISTS (SELECT 1 FROM public.mail_disposition_write_starts AS started
                          WHERE started.work_id = work.id)
    ) THEN
        RAISE EXCEPTION 'season_closed 处置只能用于季末关闭、且从未起写的寄售取得投递的当前 fence'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_season_closed_disposition';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c7_season_closed_disposition_binding
BEFORE INSERT ON public.delivery_dispositions
FOR EACH ROW WHEN (NEW.kind = 'season_closed')
EXECUTE FUNCTION public.c7_check_season_closed_disposition();

-- ⑱ 7A-3：作废的付款提醒以预检退役收口（本节追加在授权重放之前）。
-- 预检退役失败码闭集新增 consignment_notice_invalidated：付款提醒在进入 sending 前被派前复验作废（已付、
-- 已取消、轮已变）时，领取前扫描在同一事务内把从未进入 sending 的投递写成 failed，并追加 preflight_retired
-- 处置（resolves_alert = true，统筹方任务书 P7A3：分类口径不计为未处置失败）。只放宽 000088 / 000113 的两条
-- 处置 CHECK，check6 以 000088 的最终文本为底、check7 以本文件 ⑰（6D6）的文本为底，只新增这一失败码；种类闭集、来源约束与只追加触发器都不变（不变量 8）。
ALTER TABLE public.delivery_dispositions
    DROP CONSTRAINT delivery_dispositions_check6,
    DROP CONSTRAINT delivery_dispositions_check7,
    ADD CONSTRAINT delivery_dispositions_check6 CHECK (kind <> 'preflight_retired' OR (
        reason_code IS NOT NULL AND reason_code IN (
            'lot_watch_invalidated', 'recipient_renamed',
            'auction_notice_invalidated', 'season_notice_invalidated',
            'consignment_notice_invalidated'
        )
    )),
    ADD CONSTRAINT delivery_dispositions_check7 CHECK (
        (kind IN ('auto_sent_proof', 'operator_sent_confirmation', 'admin_failed',
                  'equipment_loss', 'cod_returned', 'recovery_equipment_recovered',
                  'legacy_self', 'operator_acknowledged', 'season_closed') AND resolves_alert)
        OR (kind = 'preflight_retired'
            AND resolves_alert = (reason_code IN (
                'lot_watch_invalidated', 'consignment_notice_invalidated')))
    );

-- ⑲ PIW（S2-4 / Q4 / S2-10）：整封首次决定的接管分支回执 `return_acquired`、灭失证据改由 Admin 录入、
-- 权利通知的账本回执（本节追加在授权重放之前）。
-- 可信寄售 RTS 在整封首次决定（decide_envelope）的同一事务内交退件接管写端，回执与直接接管的 acquire_return
-- 同形：带 decision_id、return_intake_id、original_round、take_work_id、take_delivery_id，不带挂单变更与隔离引用。
-- 以 ⑯ 的包装函数为底（改名 c7_valid_v2_receipt_e2，文本不动），新函数只接管这一成功分支，其余与拒绝一律委托；
-- CHECK 绑定函数 OID，所以重建回执约束。缺字段一律失败关闭。
ALTER FUNCTION public.c4_valid_v2_receipt(jsonb, uuid, text, integer)
    RENAME TO c7_valid_v2_receipt_e2;

CREATE FUNCTION public.c4_valid_v2_receipt(
    body jsonb, command_id uuid, operation text, status integer
) RETURNS boolean
LANGUAGE plpgsql STABLE PARALLEL SAFE SET search_path = pg_catalog, public
AS $function$
BEGIN
    IF status >= 400 OR NOT (
           (operation = 'decide_envelope'
                AND COALESCE(body->>'disposition' = 'return_acquired', false))
           OR operation = 'deliver_right_notice') THEN
        RETURN public.c7_valid_v2_receipt_e2(body, command_id, operation, status);
    END IF;
    IF jsonb_typeof(body) <> 'object'
       OR body->'schema_version' IS DISTINCT FROM '2'::jsonb
       OR body->'command_id' IS DISTINCT FROM to_jsonb(command_id)
       OR body->>'operation' IS DISTINCT FROM operation THEN
        RETURN false;
    END IF;
    -- 权利通知回执只带工作 ID 与通知种类，不复现挑战码、定位符或正文（D9 §6.2 deliver_right_notice）。
    IF operation = 'deliver_right_notice' THEN
        RETURN COALESCE(status = 200
            AND body->>'disposition' IN ('notice_sent', 'notice_unknown', 'notice_suppressed')
            AND public.c4_valid_json_uuid(body->'work_id')
            AND body->>'notice_kind' IN ('locator_guide', 'challenge', 'loss_receipt', 'right_notice')
            AND NOT body ?| ARRAY['challenge', 'locator', 'token', 'sealed_payload', 'body'], false);
    END IF;
    RETURN COALESCE(status = 200
        AND public.c4_valid_json_uuid(body->'decision_id')
        AND public.c4_valid_json_uuid(body->'return_intake_id')
        AND public.c4_valid_json_version(body->'original_round')
        AND public.c4_valid_json_uuid(body->'take_work_id')
        AND public.c4_valid_json_uuid(body->'take_delivery_id')
        AND (NOT body ? 'listing_changes' OR body->'listing_changes' = '[]'::jsonb)
        AND (NOT body ? 'quarantine' OR body->'quarantine' = 'null'::jsonb), false);
END
$function$;

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

-- 灭失证据改为 Admin 录入（rulings“PIW 规划第四节”Q4）：record_position_loss_proof 从 worker 移到 admin 操作，
-- 命令 ID 随之是 Admin 会话的 UUIDv4；其余 actor 分支与 ⑯ 的文本逐字相同。
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
    ) IS TRUE);


-- 权利通知的账本回执（S2-10，D9 §6.2 deliver_right_notice）：worker 操作，目标种类 notice_work（通知工作 ID）；
-- 以下两个 CHECK 以 ⑯ 的文本为底，只新增这一目标种类与操作分支。
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
                'receipt_key', 'buyer_cycle', 'position', 'right', 'browser', 'contract',
                'sanction', 'notice_work')
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
                    -- 浏览器目标（000137，J5）没有 UUID 目标，只有 request_target 上的 64 位十六进制绑定摘要。
                    WHEN 'browser' THEN target_id IS NULL AND target_bot_uid IS NULL
                        AND target_world_id IS NULL AND target_external_mid IS NULL
                        AND target_season_id IS NULL AND target_calendar_version IS NULL
                    ELSE public.c4_valid_uuid(target_id)
                        AND target_bot_uid IS NULL AND target_world_id IS NULL
                        AND target_external_mid IS NULL AND target_season_id IS NULL
                        AND target_calendar_version IS NULL
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
                                      'purchase', 'reprice', 'relist',
                                      'fulfill_withdraw_intent', 'auto_relist'))
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
                    AND operation IN ('start_return_take', 'consume_return', 'resolve_return'))
                OR (target_kind = 'receipt_key'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation = 'register_opening_attribution')
                OR (target_kind = 'obligation'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation IN ('certify_cod', 'prove_handoff', 'settle_cash',
                                      'convert_refund'))
                OR (target_kind = 'buyer_cycle'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation IN ('open_sanction', 'reset_cycle'))
                OR (target_kind = 'position'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation IN ('confirm_position_loss', 'record_loss_case_outcome',
                                      'record_position_loss_proof'))
                OR (target_kind = 'right'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation = 'claim_right')
                OR (target_kind = 'contract'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation IN ('remind', 'recheck', 'resolve'))
                OR (target_kind = 'sanction'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation = 'apply_sanction')
                OR (target_kind = 'notice_work'
                    AND request_target = target_id::text AND listing_id IS NULL
                    AND operation = 'deliver_right_notice')
                OR (target_kind = 'browser'
                    AND request_target COLLATE "C" ~ '^[0-9a-f]{64}$' AND listing_id IS NULL
                    AND operation IN ('locate_right', 'challenge_right', 'verify_right'))
            )
        )
    );

-- 凭据域密钥轮换（维护者 2026-11-25 决定：迁移到新密钥）：`keys reencrypt --domain credential` 以当前数据钥
-- 重封 outbox 里仍带密文的行，照预约码、邮件证据信封的写法以运行账号按列 CAS 改写。受控分支只放行
-- “密文三列整体换新、其余列（含租约列）逐字不变”的更新：钥标识与 nonce 都必须换新，载荷长度不变
-- （同一挑战码重封，长度只由码长与指纹决定）；明文与 AAD 由 CLI 解封复核，库内不可见。已有结果的行同样
-- 放行：密文仍在库中，旧钥退役前必须换封，且分支不动租约，不构成再次领取。其余更新照旧走原守卫。
-- 不变量 1、4、5、7：身份、目标与租约不因换钥改变，结果未知仍不重发，秘密不以旧钥长期滞留。
CREATE OR REPLACE FUNCTION public.c7_guard_notice_outbox() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF OLD.sealed_payload IS NOT NULL
       AND ROW(NEW.work_id, NEW.notice_kind, NEW.challenge_id, NEW.right_id, NEW.loss_case_id,
               NEW.frozen_identity, NEW.world_id, NEW.claim_token, NEW.claim_fence,
               NEW.lease_expires_at, NEW.created_at)
           IS NOT DISTINCT FROM
           ROW(OLD.work_id, OLD.notice_kind, OLD.challenge_id, OLD.right_id, OLD.loss_case_id,
               OLD.frozen_identity, OLD.world_id, OLD.claim_token, OLD.claim_fence,
               OLD.lease_expires_at, OLD.created_at)
       AND NEW.encryption_key_id IS DISTINCT FROM OLD.encryption_key_id
       AND NEW.nonce IS DISTINCT FROM OLD.nonce
       AND octet_length(NEW.sealed_payload) = octet_length(OLD.sealed_payload) THEN
        RETURN NEW;
    END IF;
    IF ROW(NEW.work_id, NEW.notice_kind, NEW.challenge_id, NEW.right_id, NEW.loss_case_id,
           NEW.frozen_identity, NEW.world_id, NEW.sealed_payload, NEW.nonce,
           NEW.encryption_key_id, NEW.created_at)
       IS DISTINCT FROM
       ROW(OLD.work_id, OLD.notice_kind, OLD.challenge_id, OLD.right_id, OLD.loss_case_id,
           OLD.frozen_identity, OLD.world_id, OLD.sealed_payload, OLD.nonce,
           OLD.encryption_key_id, OLD.created_at) THEN
        RAISE EXCEPTION '权利通知 outbox 只允许更新租约列'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_notice_immutable';
    END IF;
    IF NEW.claim_fence < OLD.claim_fence OR NEW.claim_fence > OLD.claim_fence + 1 THEN
        RAISE EXCEPTION '权利通知租约围栏必须单调且每次至多加一'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_notice_fence';
    END IF;
    IF EXISTS (
        SELECT 1 FROM public.consignment_right_notice_results AS result
        WHERE result.work_id = OLD.work_id
    ) THEN
        RAISE EXCEPTION '已有结果的权利通知不得再次领取'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_notice_finished';
    END IF;
    RETURN NEW;
END
$function$;

-- ⑳ PIWS4：订单复核工作的领取与完成（维护者 11-25 决定“订单复核采用方案 A”；本节追加在授权重放之前）。
-- 复核只针对该订单“已起写、无终局证据”的派发尝试（起写无 Sent 绑定）与已起写未证的 Take（起写无结论性证明），
-- 按原 attempt 重读核对；补记证据一律经既有写口，本节不新增证据写口，也不产生任何起写或外写许可。
-- ⑯ 的复核工作行原为只追加；这里给它补租约列（领取 token、fence、租约到期与领取时冻结的待核对项快照），
-- 完成结果另表只追加（结果闭集 bound / still_unknown / nothing_to_check），满足不变量 5：工作项、租约 / fence、
-- 完成时刻与结果均持久，内存定时器只用于唤醒。
-- 不变量 1、4、5、6、8：身份列不可改；每次领取必须换新 token 且 fence 恰好加一、旧租约已过期；
-- 已有结果的工作不得再领取；结果只追加、每个工作至多一条，且必须由当前租约写入。
ALTER TABLE public.consignment_order_rechecks
    ADD COLUMN claim_token uuid,
    ADD COLUMN claim_fence bigint NOT NULL DEFAULT 0,
    ADD COLUMN lease_expires_at timestamptz,
    ADD COLUMN pending_dispatch_attempts uuid[] NOT NULL DEFAULT '{}',
    ADD COLUMN pending_take_works uuid[] NOT NULL DEFAULT '{}',
    ADD CONSTRAINT c7_recheck_lease_ck CHECK (
        claim_fence BETWEEN 0 AND 9007199254740991
        AND ((claim_token IS NULL) = (lease_expires_at IS NULL)) IS TRUE
        AND ((claim_token IS NULL) = (claim_fence = 0)) IS TRUE
        AND (lease_expires_at IS NULL OR isfinite(lease_expires_at)) IS TRUE
    ),
    -- 快照只在领取时写入：未领取时为空；元素非空、不重复（核对项按 attempt / Take 工作 ID 标识）。
    ADD CONSTRAINT c7_recheck_pending_ck CHECK (
        (claim_token IS NOT NULL
            OR (cardinality(pending_dispatch_attempts) = 0 AND cardinality(pending_take_works) = 0))
        AND array_position(pending_dispatch_attempts, NULL) IS NULL
        AND array_position(pending_take_works, NULL) IS NULL
        AND cardinality(pending_dispatch_attempts) <= 1000
        AND cardinality(pending_take_works) <= 1000
    );

CREATE TABLE public.consignment_order_recheck_results (
    work_id uuid NOT NULL CONSTRAINT c7_recheck_result_pk PRIMARY KEY
        CONSTRAINT c7_recheck_result_work_fk
        REFERENCES public.consignment_order_rechecks(work_id) ON DELETE RESTRICT,
    result text NOT NULL,
    claim_fence bigint NOT NULL,
    completed_at timestamptz NOT NULL,
    CONSTRAINT c7_recheck_result_ck CHECK (result IN ('bound', 'still_unknown', 'nothing_to_check')),
    CONSTRAINT c7_recheck_result_fence_ck CHECK (claim_fence BETWEEN 1 AND 9007199254740991),
    CONSTRAINT c7_recheck_result_time_ck CHECK (isfinite(completed_at))
);

-- 工作行只有租约列可变：身份列不变；每次更新都是一次新领取（新 token、fence 恰好加一、旧租约已过期）；
-- 已有结果的工作不得再领取（不变量 4、5）。
CREATE FUNCTION public.c7_guard_order_recheck() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF ROW(NEW.work_id, NEW.contract_id, NEW.contract_version, NEW.requested_by, NEW.command_id,
           NEW.requested_at)
       IS DISTINCT FROM
       ROW(OLD.work_id, OLD.contract_id, OLD.contract_version, OLD.requested_by, OLD.command_id,
           OLD.requested_at) THEN
        RAISE EXCEPTION '订单复核工作只允许更新租约列'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_recheck_immutable';
    END IF;
    IF NEW.claim_fence IS DISTINCT FROM OLD.claim_fence + 1
       OR NEW.claim_token IS NULL
       OR NEW.claim_token IS NOT DISTINCT FROM OLD.claim_token THEN
        RAISE EXCEPTION '订单复核每次领取必须换新 token 且 fence 恰好加一'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_recheck_fence';
    END IF;
    IF OLD.lease_expires_at > clock_timestamp() THEN
        RAISE EXCEPTION '订单复核的当前租约尚未过期'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_recheck_leased';
    END IF;
    IF EXISTS (
        SELECT 1 FROM public.consignment_order_recheck_results AS result
        WHERE result.work_id = OLD.work_id
    ) THEN
        RAISE EXCEPTION '已有结果的订单复核不得再次领取'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_recheck_finished';
    END IF;
    RETURN NEW;
END
$function$;

-- ⑯ 的“拒 UPDATE / DELETE”触发器换成租约守卫加拒 DELETE；拒 TRUNCATE 不变。
DROP TRIGGER c7_consignment_order_rechecks_append_only ON public.consignment_order_rechecks;
CREATE TRIGGER c7_order_recheck_guard
BEFORE UPDATE ON public.consignment_order_rechecks
FOR EACH ROW EXECUTE FUNCTION public.c7_guard_order_recheck();
CREATE TRIGGER c7_consignment_order_rechecks_no_delete
BEFORE DELETE ON public.consignment_order_rechecks
FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation();

-- 结果只由当前租约写入：工作行已领取、fence 与结果一致、租约未过期；nothing_to_check 当且仅当领取快照为空，
-- bound 要求快照里每一项此刻都已有终局证据（派发尝试有 Sent 绑定、Take 有结论性证明）。
-- 应用层按同一判据推导结果，这里是数据库最后一道门（不变量 4、6）。
CREATE FUNCTION public.c7_guard_order_recheck_result() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
DECLARE
    work public.consignment_order_rechecks%ROWTYPE;
    snapshot_empty boolean;
    open_items boolean;
BEGIN
    SELECT * INTO work FROM public.consignment_order_rechecks WHERE work_id = NEW.work_id;
    IF NOT FOUND OR work.claim_token IS NULL OR work.claim_fence <> NEW.claim_fence
       OR NOT (work.lease_expires_at > clock_timestamp()) THEN
        RAISE EXCEPTION '订单复核结果必须由当前有效租约写入'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_recheck_result_lease';
    END IF;
    snapshot_empty := cardinality(work.pending_dispatch_attempts) = 0
        AND cardinality(work.pending_take_works) = 0;
    IF (NEW.result = 'nothing_to_check') IS DISTINCT FROM snapshot_empty THEN
        RAISE EXCEPTION 'nothing_to_check 当且仅当领取时没有待核对项'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_recheck_result_scope';
    END IF;
    IF NEW.result = 'bound' THEN
        open_items := EXISTS (
            SELECT 1 FROM public.consignment_dispatch_attempts AS attempt
            WHERE attempt.attempt_id = ANY (work.pending_dispatch_attempts)
              AND NOT EXISTS (
                  SELECT 1 FROM public.delivery_sent_bindings AS sent
                  WHERE sent.delivery_id = attempt.delivery_id
                    AND sent.fencing_version = attempt.fence))
        OR EXISTS (
            SELECT 1 FROM unnest(work.pending_take_works) AS item(take_work_id)
            WHERE NOT EXISTS (
                SELECT 1 FROM public.mail_disposition_proofs AS proof
                WHERE proof.work_id = item.take_work_id
                  AND proof.conclusion IN ('take_proven', 'return_proven')));
        IF open_items THEN
            RAISE EXCEPTION 'bound 要求领取快照里的每一项都已有终局证据'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_recheck_result_bound';
        END IF;
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c7_order_recheck_result_binding
BEFORE INSERT ON public.consignment_order_recheck_results
FOR EACH ROW EXECUTE FUNCTION public.c7_guard_order_recheck_result();
CREATE TRIGGER c7_consignment_order_recheck_results_append_only
BEFORE UPDATE OR DELETE ON public.consignment_order_recheck_results
FOR EACH ROW EXECUTE FUNCTION public.consignment_reject_fact_mutation();
CREATE TRIGGER c7_consignment_order_recheck_results_no_truncate
BEFORE TRUNCATE ON public.consignment_order_recheck_results
FOR EACH STATEMENT EXECUTE FUNCTION public.consignment_reject_fact_mutation();

-- ㉑ PIWS5：寄售 record_only 载体首封的 record_carrier 来源对真实派发轮载荷可验（本节追加在授权重放之前）。
-- 000136 的 settlement_record_carrier_consignment_valid_000136 按通用投递载荷要求 `equipment_secret_refs = []`；
-- 寄售派发轮的投递载荷（首轮与补寄同形）只带 `equipment_ids`、没有该键，判定恒为假，record_only 的载体起写
-- 因而一律失败关闭。这里整函数重建，只把“无附件”判据改为：两个键至少出现一个，且出现的键都是空数组；
-- 其余条件逐字不变（不变量 6：仍只认 isekai 载体义务在 main 的冻结派发轮、1 Credit、COD P+1、同合同同义务的
-- carrier 分配，账户与成交都冻结为 record_only）。CREATE OR REPLACE 保留函数 OID，调用它的 issue 上限触发器不变。
CREATE OR REPLACE FUNCTION public.settlement_record_carrier_consignment_valid_000136(
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
          AND (delivery.payload ? 'equipment_ids' OR delivery.payload ? 'equipment_secret_refs')
          AND COALESCE(delivery.payload->'equipment_ids', '[]'::jsonb) = '[]'::jsonb
          AND COALESCE(delivery.payload->'equipment_secret_refs', '[]'::jsonb) = '[]'::jsonb
    );
$function$;

-- ㉓ PIT2：与 c7_sale_closure_paid 相反方向的纵深防御：已按未付取消封闭的 Sale 不得再写 funded 投影
-- （不变量 6）。应用层 certify_cod 已在资金账户锁后先拒；这里是数据库最后一道门。
CREATE FUNCTION public.c7_guard_funded_after_closure() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog
AS $function$
BEGIN
    IF EXISTS (
        SELECT 1 FROM public.consignment_sale_closures AS closure
        WHERE closure.contract_id = NEW.contract_id AND closure.closure_kind = 'cancelled_unpaid'
    ) THEN
        RAISE EXCEPTION '已按未付取消封闭的 Sale 不能再记实收'
            USING ERRCODE = '23514', CONSTRAINT = 'c7_funded_after_unpaid_closure';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER c7_funded_after_unpaid_closure
BEFORE INSERT ON public.consignment_funded_sales
FOR EACH ROW EXECUTE FUNCTION public.c7_guard_funded_after_closure();

-- ㉒ PIWX：撤回件派发的 position 边（统筹方裁定，本节追加在授权重放之前）。撤回事务同提撤回件首轮后，
-- 原件腿首次起写把 position 从 in_custody 推进到 in_transit，被退回并精确消费后回到 in_custody；两条边都以
-- “当前处分代的合同是 Withdrawal”加同事务的原件腿尝试 / 消费为守卫。c4_guard_position 整函数以 ⑪（7C-3）的
-- 文本为底，只新增这两个分支，并把既有 committed <-> in_transit 两个 Sale 分支的合同限定为 kind = 'sale'
-- （撤回件从不经过 committed，收紧不改变 Sale 的任何结果）；代管事件 CHECK 以 ⑪ 为底，只给 in_custody
-- 事件加来源 in_transit、给 in_transit 事件加来源 in_custody。不变量 2、4、6。
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
    IF NEW.state = OLD.state AND NEW.current_generation IS DISTINCT FROM OLD.current_generation THEN
        -- 处分代指针换代（000137，D9 §5.3 撤回 3 / 重列）：状态不变，指针只能前进到 g+1，且同事务写入了
        -- (position, g+1) 代行（predecessor=g，purpose 为 relist / withdrawal_only）；其成功命令与合同由
        -- c7_check_generation_successor 在 COMMIT 前核对（7C-3 修正：命令账本行按 DepositWriter 的写序最后
        -- 才写入，行级守卫在指针更新时看不到它）；
        -- 跳代、回退与没有后继行一律拒绝（不变量 2、6）。只在原物已取得或收回的 in_custody / committed 放行
        -- （D9 §5.3 撤回 2：旧买方交割已封闭）；in_transit、reconcile 的交割未封闭或未知，不得换代。
        IF OLD.state NOT IN ('in_custody', 'committed')
           OR OLD.current_generation IS NULL
           OR NEW.current_generation IS DISTINCT FROM OLD.current_generation + 1
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_disposition_generations AS successor
               WHERE successor.position_id = NEW.id
                 AND successor.generation = NEW.current_generation
                 AND successor.predecessor = OLD.current_generation
                 AND successor.purpose IN ('relist', 'withdrawal_only')
                 AND successor.xmin = pg_current_xact_id()::xid
           ) THEN
            RAISE EXCEPTION '处分代指针只能前进到同事务写入的唯一后继代（g+1）'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_position_generation_pointer';
        END IF;
    ELSIF OLD.state = 'awaiting_take' AND NEW.state = 'in_custody' THEN
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
    ELSIF OLD.state IN ('in_custody', 'committed', 'in_transit', 'reconcile')
          AND NEW.state = 'season_ended' THEN
        -- 已承诺、在途与核对中的件同样在季窗结束后终结（000137，J8）；退款与交付责任不在此处理。
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
        -- 资产腿的首次起写：同事务写入该合同原件腿的派发尝试。合同限 Sale（PIWX：撤回件不经 committed）。
        IF NEW.current_generation IS DISTINCT FROM OLD.current_generation
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_dispatch_attempts AS attempt
               JOIN public.consignment_contracts AS contract ON contract.contract_id = attempt.contract_id
               WHERE contract.position_id = NEW.id AND contract.generation = NEW.current_generation
                 AND contract.kind = 'sale'
                 AND attempt.leg = 'original' AND attempt.xmin = pg_current_xact_id()::xid
           ) THEN
            RAISE EXCEPTION '在途必须同事务写入原件腿的派发尝试'
                USING ERRCODE = '23514', CONSTRAINT = 'c6_position_transit_fact';
        END IF;
    ELSIF OLD.state = 'in_transit' AND NEW.state = 'committed' THEN
        -- 原件被退回并消费：同事务写入该合同原件腿的退件消费。合同限 Sale（PIWX：撤回件回到 in_custody）。
        IF NEW.current_generation IS DISTINCT FROM OLD.current_generation
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_return_consumptions AS consumption
               JOIN public.consignment_contracts AS contract
                 ON contract.contract_id = consumption.contract_id
               WHERE contract.position_id = NEW.id AND contract.generation = NEW.current_generation
                 AND contract.kind = 'sale'
                 AND consumption.leg = 'original' AND consumption.xmin = pg_current_xact_id()::xid
           ) THEN
            RAISE EXCEPTION '原件退回必须同事务写入原件腿的退件消费'
                USING ERRCODE = '23514', CONSTRAINT = 'c6_position_return_fact';
        END IF;
    ELSIF OLD.state = 'in_custody' AND NEW.state = 'in_transit' THEN
        -- 撤回件首次起写（PIWX）：当前处分代的合同必须是 Withdrawal，且同事务写入该合同原件腿的派发尝试；
        -- 处分代不变。Sale 与无合同的代不得借此边（不变量 2、4、6）。
        IF NEW.current_generation IS DISTINCT FROM OLD.current_generation
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_dispatch_attempts AS attempt
               JOIN public.consignment_contracts AS contract ON contract.contract_id = attempt.contract_id
               WHERE contract.position_id = NEW.id AND contract.generation = NEW.current_generation
                 AND contract.kind = 'withdrawal'
                 AND attempt.leg = 'original' AND attempt.xmin = pg_current_xact_id()::xid
           ) THEN
            RAISE EXCEPTION '撤回件在途必须同事务写入当前代 Withdrawal 原件腿的派发尝试'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_position_withdrawal_transit_fact';
        END IF;
    ELSIF OLD.state = 'in_transit' AND NEW.state = 'in_custody' THEN
        -- 撤回件被退回并精确消费（PIWX）：当前处分代的合同必须是 Withdrawal，且同事务写入该合同原件腿的
        -- 退件消费；原件回库、处分代不变。Sale 的原件退回仍回 committed（不变量 2、4、6）。
        IF NEW.current_generation IS DISTINCT FROM OLD.current_generation
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_return_consumptions AS consumption
               JOIN public.consignment_contracts AS contract
                 ON contract.contract_id = consumption.contract_id
               WHERE contract.position_id = NEW.id AND contract.generation = NEW.current_generation
                 AND contract.kind = 'withdrawal'
                 AND consumption.leg = 'original' AND consumption.xmin = pg_current_xact_id()::xid
           ) THEN
            RAISE EXCEPTION '撤回件回库必须同事务写入当前代 Withdrawal 原件腿的退件消费'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_position_withdrawal_return_fact';
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
    ELSIF OLD.state = 'committed' AND NEW.state = 'in_custody' THEN
        -- 回库（000137，7C-3）：当前处分代的 Sale 已有封闭事实，且原件从未离库或已被精确收回；同事务先写
        -- uncommitted 事件（引用该封闭事实，版本即本次新版本），处分代不变。原件是否真的在库由
        -- c7_sale_original_back 按派发尝试与退件接管核对（不变量 2、4、6）。
        IF NEW.current_generation IS DISTINCT FROM OLD.current_generation
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_custody_events AS event_row
               JOIN public.consignment_contracts AS contract
                 ON contract.contract_id = event_row.proof_id
               JOIN public.consignment_sale_closures AS closure
                 ON closure.contract_id = contract.contract_id
               WHERE event_row.position_id = NEW.id AND event_row.version = NEW.version
                 AND event_row.event_kind = 'uncommitted'
                 AND event_row.xmin = pg_current_xact_id()::xid
                 AND contract.position_id = NEW.id AND contract.generation = NEW.current_generation
                 AND contract.kind = 'sale'
                 AND public.c7_sale_original_back(contract.contract_id)
           ) THEN
            RAISE EXCEPTION '回库必须同事务引用当前代已封闭的 Sale，且原件从未离库或已被精确收回'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_position_recall_fact';
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
    ELSIF OLD.state IN ('in_custody', 'committed', 'in_transit', 'reconcile')
          AND NEW.state = 'lost' THEN
        -- 已证灭失（000137，J8）：指针留在确认时的当前代（D9 §5.3，不新增处分代），且同事务必须写入
        -- 绑定本 position 与当前代的损失事实；损失事实的外键保证它引用同 position 的损失证据。
        IF NEW.current_generation IS DISTINCT FROM OLD.current_generation
           OR NOT EXISTS (
               SELECT 1 FROM public.consignment_loss_facts AS fact
               WHERE fact.position_id = NEW.id AND fact.generation = NEW.current_generation
                 AND fact.xmin = pg_current_xact_id()::xid
           ) THEN
            RAISE EXCEPTION '灭失必须同事务写入当前处分代的损失事实且不替换处分代'
                USING ERRCODE = '23514', CONSTRAINT = 'c7_position_loss_fact';
        END IF;
    ELSE
        RAISE EXCEPTION 'position 迁移不合法'
            USING ERRCODE = '23514', CONSTRAINT = 'c4_position_transition';
    END IF;
    RETURN NEW;
END
$function$;

ALTER TABLE public.consignment_custody_events
    DROP CONSTRAINT c4_custody_event_state_ck,
    ADD CONSTRAINT c4_custody_event_state_ck CHECK ((
        CASE event_kind
            WHEN 'accepted' THEN from_state IS NULL AND to_state = 'awaiting_take'
            WHEN 'in_custody' THEN from_state IN ('awaiting_take', 'reconcile', 'in_transit')
                AND to_state = 'in_custody'
            WHEN 'reconcile' THEN from_state IN ('in_custody', 'committed', 'in_transit')
                AND to_state = 'reconcile'
            WHEN 'committed' THEN from_state IN ('in_custody', 'in_transit', 'reconcile')
                AND to_state = 'committed'
            WHEN 'in_transit' THEN from_state IN ('committed', 'reconcile', 'in_custody')
                AND to_state = 'in_transit'
            WHEN 'handed_over' THEN from_state IN ('in_transit', 'committed')
                AND to_state = 'handed_over'
            WHEN 'season_ended' THEN from_state IS NOT NULL AND to_state = 'season_ended'
            WHEN 'lost' THEN from_state IN ('in_custody', 'committed', 'in_transit', 'reconcile')
                AND to_state = 'lost'
            -- 回库：Sale 封闭后原件仍在库，committed 回到 in_custody（000137，7C-3）。
            WHEN 'uncommitted' THEN from_state = 'committed' AND to_state = 'in_custody'
        END
    ) IS TRUE);

REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM PUBLIC, auction_runtime;
SELECT public.apply_auction_runtime_privileges();

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (137, 'breaking');
