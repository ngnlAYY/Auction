//! COD 实收的冻结事实读取、互校与收据落库。
//!
//! 保护不变量 1、6：同一经济实收（原 UID、main、原付款 Sent MID、COD 腿）只有一份结果；命令与冻结的
//! Sale、义务、派发轮、Sent 绑定、远端付款明细任一不符即失败关闭，不静默修正。

use auction_application::settlement::{
    CertifyCod, CodLeg, FundingError, FundingMutation, ReceiptKey,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::dispatch::EvidenceCategory;
use auction_domain::consignment::funding::{CodReceiptSplit, FundingMode};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::opening_slice::rule;
use crate::database::TrackedTransaction as Transaction;
use crate::database::consignment::dispatch::claim_proof::category_signed;
use crate::database::settlement::funding::storage;

/// 实收业务键 ID 的 v5 域前缀；版本、边界与来源类型不参与（D9 §4.2）。
const RECEIPT_KEY_DOMAIN: &[u8] = b"auction.consignment.receipt-key.v1\0";

/// 按 SQL 错误的约束名分类：唯一键与寄售资金约束的违反是确定的业务冲突，其余沿资金错误映射。
pub(super) fn funding_sql_error(error: sqlx::Error) -> FundingError {
    if let Some(database) = error.as_database_error() {
        match (database.code().as_deref(), database.constraint()) {
            (Some("23505"), _) | (Some("23514"), Some("c6_slice_overlap")) => {
                return FundingError::Conflict;
            }
            (Some("23514"), Some(constraint)) if constraint.starts_with("c6_") => {
                return FundingError::Unproven;
            }
            _ => {}
        }
    }
    storage(error)
}

/// 实收业务键的确定性 ID：字段长度前缀，避免拼接歧义。
pub(super) fn receipt_key_id(key: &ReceiptKey) -> Uuid {
    let mut bytes = RECEIPT_KEY_DOMAIN.to_vec();
    for field in [key.uid.as_str(), "main", key.payment_mail_id.as_str(), leg_code(key.leg)] {
        bytes.extend_from_slice(&u64::try_from(field.len()).unwrap_or(u64::MAX).to_be_bytes());
        bytes.extend_from_slice(field.as_bytes());
    }
    Uuid::new_v5(&Uuid::NAMESPACE_URL, &bytes)
}

/// COD 腿的持久码。
pub(super) const fn leg_code(leg: CodLeg) -> &'static str {
    match leg {
        CodLeg::Original => "original",
        CodLeg::Carrier => "carrier",
    }
}

/// 世界的持久码。
const fn world_code(world: SettlementWorld) -> &'static str {
    match world {
        SettlementWorld::Main => "main",
        SettlementWorld::Isekai => "isekai",
    }
}

/// 资金模式的持久码。
pub(super) const fn mode_code(mode: FundingMode) -> &'static str {
    match mode {
        FundingMode::StrictCash => "strict_cash",
        FundingMode::RecordOnly => "record_only",
    }
}

/// 锁内读出的冻结事实：Sale、义务、履约、派发轮、Sent 绑定与远端付款明细。
#[derive(Debug)]
pub(super) struct ReceiptFacts {
    contract_world: String,
    price: i64,
    fee: i64,
    seller_amount: i64,
    buyer_user_id: Uuid,
    funding_uid: String,
    funding_mode: String,
    obligation_kind: String,
    obligation_world: String,
    obligation_amount: i64,
    carrier_credit: i64,
    recipient_user_id: Option<Uuid>,
    business_state: String,
    execution_state: String,
    fulfillment_delivery: Option<Uuid>,
    delivery_operation: String,
    round_bot_uid: String,
    round_world: String,
    round_cod: i64,
    pub(super) round_number: i32,
    pub(super) attempt_id: Uuid,
    send_after_boundary: bool,
    pub(super) before_season_end: Option<bool>,
}

/// 已锁定的合同义务图中后续步骤要用的两条义务：卖款与（isekai）原件交接。
#[derive(Debug, Clone, Copy)]
pub(super) struct ContractObligations {
    pub(super) payout: Option<Uuid>,
    pub(super) handoff: Option<Uuid>,
}

/// 远端付款明细：派发尝试上唯一一条对应腿的付款 / 确领证据。
#[derive(Debug)]
pub(super) struct RemotePayment {
    pub(super) evidence_id: Uuid,
    pub(super) reference: String,
}

/// 锁定本合同的全部义务并读出卖款与原件交接义务（锁序第一段：obligations，之后才是 allocations）。
pub(super) async fn lock_obligations(
    tx: &mut Transaction<'_, Postgres>,
    command: &CertifyCod,
) -> Result<ContractObligations, FundingError> {
    let rows = sqlx::query(
        r#"SELECT id, obligation_kind FROM public.settlement_obligations
           WHERE contract_id=$1 ORDER BY id FOR UPDATE"#,
    )
    .bind(command.contract_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(storage)?;
    let mut found = ContractObligations { payout: None, handoff: None };
    for row in &rows {
        let id: Uuid = row.try_get("id").map_err(storage)?;
        match row.try_get::<String, _>("obligation_kind").map_err(storage)?.as_str() {
            "seller_payout" => found.payout = Some(id),
            "winner_equipment_handoff" => found.handoff = Some(id),
            _ => {}
        }
    }
    Ok(found)
}

/// 锁定原付款 delivery 与履约行（锁序末段：deliveries → fulfillments，必须在全部 allocations 之后）。
pub(super) async fn lock_delivery_rows(
    tx: &mut Transaction<'_, Postgres>,
    command: &CertifyCod,
) -> Result<(), FundingError> {
    sqlx::query("SELECT id FROM public.deliveries WHERE id=$1 FOR UPDATE")
        .bind(command.original_delivery_id)
        .execute(&mut **tx)
        .await
        .map_err(storage)?;
    sqlx::query(
        r#"SELECT obligation_id FROM public.settlement_obligation_fulfillments
           WHERE obligation_id IN (
               SELECT id FROM public.settlement_obligations WHERE contract_id=$1)
           ORDER BY obligation_id FOR UPDATE"#,
    )
    .bind(command.contract_id)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(())
}

/// 读取冻结事实；任一连接缺失说明命令引用了不存在或未绑定的事实，按未证明失败关闭。
pub(super) async fn load_facts(
    tx: &mut Transaction<'_, Postgres>,
    command: &CertifyCod,
) -> Result<ReceiptFacts, FundingError> {
    let row = sqlx::query(
        r#"SELECT contract.world_id AS contract_world,
                  sale.price, sale.fee, sale.seller_amount, sale.buyer_user_id,
                  sale.funding_uid, sale.funding_mode,
                  obligation.obligation_kind, obligation.world_id AS obligation_world,
                  obligation.amount AS obligation_amount,
                  obligation.carrier_credit::bigint AS carrier_credit,
                  obligation.recipient_user_id, obligation.business_state,
                  fulfillment.execution_state,
                  fulfillment.delivery_id AS fulfillment_delivery,
                  fulfillment.delivery_operation,
                  round.bot_uid AS round_bot_uid, round.world_id AS round_world,
                  round.cod AS round_cod, round.round AS round_number,
                  attempt.attempt_id,
                  send.sending_at > sale.funding_boundary_at AS send_after_boundary,
                  CASE WHEN window_row.end_utc IS NULL THEN NULL
                       ELSE clock_timestamp() < window_row.end_utc END AS before_season_end
           FROM public.consignment_sales AS sale
           JOIN public.consignment_contracts AS contract ON contract.contract_id = sale.contract_id
           JOIN public.settlement_obligations AS obligation
             ON obligation.id = $2 AND obligation.contract_id = sale.contract_id
           JOIN public.settlement_obligation_fulfillments AS fulfillment
             ON fulfillment.obligation_id = obligation.id
           JOIN public.consignment_dispatch_rounds AS round
             ON round.contract_id = sale.contract_id AND round.obligation_id = obligation.id
            AND round.delivery_id = $3
           JOIN public.delivery_sent_bindings AS binding
             ON binding.world_id = 'main' AND binding.external_mail_id = $4
            AND binding.delivery_id = round.delivery_id
           JOIN public.consignment_dispatch_attempts AS attempt
             ON attempt.delivery_id = round.delivery_id AND attempt.fence = binding.fencing_version
           JOIN public.delivery_send_attempts AS send
             ON send.delivery_id = attempt.delivery_id AND send.fencing_version = attempt.fence
           LEFT JOIN public.consignment_season_windows AS window_row
             ON window_row.id = contract.season_window_id
           WHERE sale.contract_id = $1"#,
    )
    .bind(command.contract_id)
    .bind(command.payment_obligation_id)
    .bind(command.original_delivery_id)
    .bind(&command.receipt.payment_mail_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or(FundingError::Unproven)?;
    let decode = storage;
    Ok(ReceiptFacts {
        contract_world: row.try_get("contract_world").map_err(decode)?,
        price: row.try_get("price").map_err(decode)?,
        fee: row.try_get("fee").map_err(decode)?,
        seller_amount: row.try_get("seller_amount").map_err(decode)?,
        buyer_user_id: row.try_get("buyer_user_id").map_err(decode)?,
        funding_uid: row.try_get("funding_uid").map_err(decode)?,
        funding_mode: row.try_get("funding_mode").map_err(decode)?,
        obligation_kind: row.try_get("obligation_kind").map_err(decode)?,
        obligation_world: row.try_get("obligation_world").map_err(decode)?,
        obligation_amount: row.try_get("obligation_amount").map_err(decode)?,
        carrier_credit: row.try_get("carrier_credit").map_err(decode)?,
        recipient_user_id: row.try_get("recipient_user_id").map_err(decode)?,
        business_state: row.try_get("business_state").map_err(decode)?,
        execution_state: row.try_get("execution_state").map_err(decode)?,
        fulfillment_delivery: row.try_get("fulfillment_delivery").map_err(decode)?,
        delivery_operation: row.try_get("delivery_operation").map_err(decode)?,
        round_bot_uid: row.try_get("round_bot_uid").map_err(decode)?,
        round_world: row.try_get("round_world").map_err(decode)?,
        round_cod: row.try_get("round_cod").map_err(decode)?,
        round_number: row.try_get("round_number").map_err(decode)?,
        attempt_id: row.try_get("attempt_id").map_err(decode)?,
        send_after_boundary: row.try_get("send_after_boundary").map_err(decode)?,
        before_season_end: row.try_get("before_season_end").map_err(decode)?,
    })
}

impl ReceiptFacts {
    /// 命令必须逐项等于冻结事实（不变量 6）：冻结金额、世界与腿、UID 与模式、付款人、履约与派发状态、
    /// 原 Send 严格晚于开户边界。不符是冲突或未证明，不做修正。
    pub(super) fn verify(&self, command: &CertifyCod) -> Result<(), FundingError> {
        let world = world_code(command.world);
        let (kind, principal) = match command.receipt.leg {
            CodLeg::Original => ("winner_equipment_cod", 0),
            CodLeg::Carrier => ("winner_payment_carrier_cod", 1),
        };
        if self.contract_world != world
            || self.obligation_world != world
            || self.round_world != "main"
        {
            return Err(FundingError::Conflict);
        }
        if self.obligation_kind != kind
            || self.carrier_credit != principal
            || self.obligation_amount != self.price
        {
            return Err(FundingError::Conflict);
        }
        if self.price != command.price.value()
            || self.fee != command.quote.fee
            || self.seller_amount != command.quote.seller_amount
        {
            return Err(FundingError::Conflict);
        }
        let gross = self.price.checked_add(self.carrier_credit).ok_or(FundingError::Invalid)?;
        if command.gross != gross || self.round_cod != gross {
            return Err(FundingError::Conflict);
        }
        if self.funding_uid != command.receipt.uid
            || self.round_bot_uid != command.receipt.uid
            || self.funding_mode != mode_code(command.mode)
        {
            return Err(FundingError::Conflict);
        }
        if self.recipient_user_id != Some(self.buyer_user_id) {
            return Err(FundingError::Conflict);
        }
        if self.business_state != "ready"
            || self.execution_state != "enqueued"
            || self.delivery_operation != "send"
            || self.fulfillment_delivery != Some(command.original_delivery_id)
        {
            return Err(FundingError::Unproven);
        }
        if !self.send_after_boundary {
            return Err(FundingError::Unproven);
        }
        Ok(())
    }
}

/// 开户边界链复验：Sale 冻结的边界来源仍是该 UID 的 opening 来源，且边界之后没有另一次 opening；
/// 链变化（调整、结转、重开）时原 Send 不再能被原边界证明，整笔待核。
pub(super) async fn verify_boundary_chain(
    tx: &mut Transaction<'_, Postgres>,
    command: &CertifyCod,
) -> Result<(), FundingError> {
    let intact: bool = sqlx::query_scalar(
        r#"SELECT (sale.funding_boundary_source_id IS NULL OR EXISTS (
                       SELECT 1 FROM public.settlement_funding_sources AS boundary
                       WHERE boundary.id = sale.funding_boundary_source_id
                         AND boundary.uid = sale.funding_uid
                         AND boundary.kind IN ('reconciliation', 'record_opening')))
                  AND NOT EXISTS (
                       SELECT 1 FROM public.settlement_funding_sources AS later
                       WHERE later.uid = sale.funding_uid
                         AND later.kind IN ('reconciliation', 'record_opening')
                         AND later.created_at > sale.funding_boundary_at
                         AND later.id IS DISTINCT FROM sale.funding_boundary_source_id)
           FROM public.consignment_sales AS sale
           WHERE sale.contract_id = $1"#,
    )
    .bind(command.contract_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    if intact { Ok(()) } else { Err(FundingError::Conflict) }
}

/// 读取原付款腿的远端付款 / 确领明细；缺失说明付款未经证明（record_only 同样适用），多条则歧义。
/// 唯一一条明细还须出自已签收的验证器（D9 §2.3 `remote_cod_paid`，PIWQ）：未签收的观察只是记录，按缺证整笔
/// 回滚、不写收据（不变量 4、6），签收后有新的已签收观察再认证。
pub(super) async fn remote_payment(
    tx: &mut Transaction<'_, Postgres>,
    command: &CertifyCod,
    attempt_id: Uuid,
) -> Result<RemotePayment, FundingError> {
    let category = match command.receipt.leg {
        CodLeg::Original => EvidenceCategory::OriginalClaim,
        CodLeg::Carrier => EvidenceCategory::CarrierPayment,
    };
    let rows = sqlx::query(
        r#"SELECT id, remote_observation_id, validator_version
           FROM public.consignment_dispatch_evidence
           WHERE attempt_id = $1 AND category = $2
           ORDER BY remote_observation_id LIMIT 2"#,
    )
    .bind(attempt_id)
    .bind(category.as_str())
    .fetch_all(&mut **tx)
    .await
    .map_err(storage)?;
    match rows.as_slice() {
        [] => Err(FundingError::Unproven),
        [row] if !category_signed(category, row.try_get("validator_version").map_err(storage)?) => {
            Err(FundingError::Unproven)
        }
        [row] => Ok(RemotePayment {
            evidence_id: row.try_get("id").map_err(storage)?,
            reference: row.try_get("remote_observation_id").map_err(storage)?,
        }),
        _ => Err(FundingError::Conflict),
    }
}

/// 已存在的实收：任一业务键（实收业务键、付款义务、付款 delivery）命中即是同一经济实收。
#[derive(Debug)]
pub(super) struct ExistingReceipt {
    pub(super) source_id: Uuid,
    contract_id: Uuid,
    payment_obligation_id: Uuid,
    delivery_id: Uuid,
    receipt_key_id: Uuid,
    gross: i64,
    pub(super) opening_included: i64,
}

impl ExistingReceipt {
    /// 相容才是重放：同合同、同义务、同 delivery、同实收业务键、同毛额与同已含份额。
    pub(super) fn is_replay_of(&self, command: &CertifyCod, key_id: Uuid, included: i64) -> bool {
        self.contract_id == command.contract_id
            && self.payment_obligation_id == command.payment_obligation_id
            && self.delivery_id == command.original_delivery_id
            && self.receipt_key_id == key_id
            && self.gross == command.gross
            && self.opening_included == included
    }

    /// 由已存收据还原拆分，供重放回执复算过渡计划。
    pub(super) fn split(&self) -> Result<CodReceiptSplit, FundingError> {
        CodReceiptSplit::new(self.gross, self.opening_included).map_err(rule)
    }
}

/// 查找同一经济实收的既有收据；命中多行（不同业务键各占一行）说明历史冲突，失败关闭。
pub(super) async fn existing_receipt(
    tx: &mut Transaction<'_, Postgres>,
    command: &CertifyCod,
    key_id: Uuid,
) -> Result<Option<ExistingReceipt>, FundingError> {
    let rows = sqlx::query(
        r#"SELECT source_id, contract_id, payment_obligation_id, delivery_id, receipt_key_id,
                  gross, opening_included
           FROM public.consignment_cod_receipts
           WHERE receipt_key_id = $1 OR payment_obligation_id = $2 OR delivery_id = $3
           LIMIT 2"#,
    )
    .bind(key_id)
    .bind(command.payment_obligation_id)
    .bind(command.original_delivery_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(storage)?;
    match rows.as_slice() {
        [] => Ok(None),
        [row] => Ok(Some(ExistingReceipt {
            source_id: row.try_get("source_id").map_err(storage)?,
            contract_id: row.try_get("contract_id").map_err(storage)?,
            payment_obligation_id: row.try_get("payment_obligation_id").map_err(storage)?,
            delivery_id: row.try_get("delivery_id").map_err(storage)?,
            receipt_key_id: row.try_get("receipt_key_id").map_err(storage)?,
            gross: row.try_get("gross").map_err(storage)?,
            opening_included: row.try_get("opening_included").map_err(storage)?,
        })),
        _ => Err(FundingError::Conflict),
    }
}

/// 本笔实收的证明摘要：只含业务键与金额，不含秘密；同一实收重放得到同一摘要。
pub(super) fn proof_digest(
    command: &CertifyCod,
    remote: &RemotePayment,
    opening_included: i64,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"auction.consignment.cod-receipt.v1\0");
    hasher.update(command.contract_id.as_bytes());
    hasher.update(command.payment_obligation_id.as_bytes());
    hasher.update(command.original_delivery_id.as_bytes());
    hasher.update(command.receipt.payment_mail_id.as_bytes());
    hasher.update([0]);
    hasher.update(remote.reference.as_bytes());
    hasher.update([0]);
    hasher.update(command.gross.to_be_bytes());
    hasher.update(opening_included.to_be_bytes());
    hasher.finalize().into()
}

/// 写入通用收据与寄售收据：付款 source 的额度就是新发行额 U，可为零（零额 source 是收据锚点）。
pub(super) async fn insert_receipt(
    tx: &mut Transaction<'_, Postgres>,
    command: &CertifyCod,
    source_id: Uuid,
    key_id: Uuid,
    remote: &RemotePayment,
    split: CodReceiptSplit,
) -> Result<(), FundingError> {
    sqlx::query(
        r#"INSERT INTO public.settlement_funding_receipts
               (source_id, uid, payment_obligation_id, delivery_id, external_mail_id, evidence)
           VALUES ($1, $2, $3, $4, $5,
                   jsonb_build_object('kind', 'consignment_cod', 'contract_id', $6::text,
                                      'leg', $7::text, 'evidence_id', $8::text))"#,
    )
    .bind(source_id)
    .bind(&command.receipt.uid)
    .bind(command.payment_obligation_id)
    .bind(command.original_delivery_id)
    .bind(&command.receipt.payment_mail_id)
    .bind(command.contract_id.to_string())
    .bind(leg_code(command.receipt.leg))
    .bind(remote.evidence_id.to_string())
    .execute(&mut **tx)
    .await
    .map_err(funding_sql_error)?;
    let written = sqlx::query(
        r#"INSERT INTO public.consignment_cod_receipts
               (source_id, uid, contract_id, payment_obligation_id, delivery_id, world_id,
                sent_mid, sent_fence, cod_leg, remote_payment_ref, receipt_key_id, gross,
                opening_included, new_issue)
           SELECT $1, $2, $3, $4, $5, 'main', $6, attempt.fence, $7, $8, $9, $10, $11, $12
           FROM public.consignment_dispatch_attempts AS attempt
           WHERE attempt.delivery_id = $5
             AND EXISTS (SELECT 1 FROM public.delivery_sent_bindings AS binding
                         WHERE binding.world_id = 'main' AND binding.external_mail_id = $6
                           AND binding.delivery_id = $5
                           AND binding.fencing_version = attempt.fence)"#,
    )
    .bind(source_id)
    .bind(&command.receipt.uid)
    .bind(command.contract_id)
    .bind(command.payment_obligation_id)
    .bind(command.original_delivery_id)
    .bind(&command.receipt.payment_mail_id)
    .bind(leg_code(command.receipt.leg))
    .bind(&remote.reference)
    .bind(key_id)
    .bind(split.gross())
    .bind(split.opening_included())
    .bind(split.new_issue())
    .execute(&mut **tx)
    .await
    .map_err(funding_sql_error)?;
    // INSERT…SELECT 在绑定不成立时静默零行；通用收据已写，必须整笔失败关闭，不留无寄售收据的来源。
    if written.rows_affected() != 1 {
        return Err(FundingError::Unproven);
    }
    Ok(())
}

/// 只登记命令账本（不改账户版本与任何分配）：同一业务对象被另一命令 ID 重放时使用，新增资金变化为零。
pub(super) async fn record_replayed_command(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    mutation: &FundingMutation,
    action: &str,
    target: Uuid,
) -> Result<(), FundingError> {
    sqlx::query(
        r#"INSERT INTO public.settlement_funding_commands
               (command_id, uid, action, target_id, request_digest, result)
           VALUES ($1, $2, $3, $4, $5, '{}')"#,
    )
    .bind(mutation.command_id)
    .bind(uid)
    .bind(action)
    .bind(target)
    .bind(mutation.request_digest.as_slice())
    .execute(&mut **tx)
    .await
    .map_err(funding_sql_error)?;
    Ok(())
}
