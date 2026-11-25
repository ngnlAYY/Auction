//! 收款与确领推进的待办来源（PIWQ）：已成功派发、已绑定 Sent、还差领取证据或下一步写口的寄售轮。
//!
//! 保护不变量 4、5、6：
//! - 本模块只读（`READ ONLY` 事务），不写证据、不认证、不确领；推进只经既有写口，由 Worker 循环调用。
//! - 待办全部来自持久事实：main 原件 COD 与 isekai 原件交割在确领证明落库前、isekai 载体 COD 在付款满足前、卖款
//!   与退款在已签收的现金确领证据落库前一直待办；同轮已接管退件（RTS）的轮不再推进（领取与退回互斥，D9 §2.3），
//!   已按未付取消或退款封闭的成交不再认证，funded 已离开 `funded` 的成交不再确领原件。
//! - 已记录的领取证据只认已签收验证器的版本：未签收的观察不当作证据交出，签收清单为空时生产上没有任何轮能
//!   拿到证据（与写口的签收判定同源，见 `claim_proof`）。
//! - 认证事实取自原派发与成交冻结值（毛额取原派发冻结的 COD），Worker 不推算金额；冻结事实缺失即跳过该轮。
//!
//! 日志只记固定分类码，不含收件人、主题、装备或 Sent MID。

use auction_application::consignment::{
    CertifyCodFacts, CollectedEvidence, CollectionCursor, CollectionLeg, CollectionPage,
    ConsignmentError, PendingCollection,
};
use auction_application::delivery::DeliverySendExpectation;
use auction_application::settlement::{CodLeg, DispatchFreeze, ReceiptKey};
use auction_domain::consignment::dispatch::{AttemptKey, DispatchRoundKey, EvidenceCategory};
use auction_domain::consignment::funding::FundingMode;
use auction_domain::consignment::{ListingPrice, ListingQuote, parse_settlement_world};
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::claim_proof::{category_signed, signed_versions};
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::mail::originals::invariant_decoding;

/// 收款与确领推进的存储来源。
#[derive(Clone)]
pub struct CollectedRoundRepository {
    store: DatabaseStore,
}

impl CollectedRoundRepository {
    /// 注入数据库。
    pub fn new(store: DatabaseStore) -> Self {
        Self { store }
    }

    /// 游标之后至多 `limit` 轮待推进的收款或确领，按 `(started_at, attempt_id)` 升序返回一页。单轮冻结事实不全
    /// 只记安全码并跳过，但同样推进游标；读满一页才给出下一页游标。读取失败返回错误。
    pub async fn pending(
        &self,
        after: Option<CollectionCursor>,
        limit: u32,
    ) -> Result<CollectionPage, ConsignmentError> {
        self.read(after, limit).await.map_err(WriteError::into_application)
    }

    /// 该类别下该验证器版本是否已签收；与写口判定同一份清单。
    pub fn validator_signed(&self, category: EvidenceCategory, version: &str) -> bool {
        category_signed(category, version)
    }

    async fn read(
        &self,
        after: Option<CollectionCursor>,
        limit: u32,
    ) -> Result<CollectionPage, WriteError> {
        let mut transaction = self.store.pool().begin_with("BEGIN READ ONLY").await?;
        let rows = pending_rows(&mut transaction, after, i64::from(limit)).await?;
        // 游标取本页最后一行（含被跳过的行）；读不满一页说明游标之后已无待办，本次扫描结束。
        let next = match rows.last() {
            Some(last) if rows.len() == usize::try_from(limit)? => Some(CollectionCursor {
                started_at: last.try_get("started_at")?,
                attempt_id: last.try_get("attempt_id")?,
            }),
            _ => None,
        };
        let mut pending = Vec::with_capacity(rows.len());
        for row in &rows {
            match collection(&mut transaction, row).await? {
                Some(item) => pending.push(item),
                None => {
                    tracing::warn!(
                        code = "collection_facts_incomplete",
                        "收款确领待办的冻结事实不全，跳过"
                    );
                }
            }
        }
        transaction.commit().await?;
        Ok(CollectionPage { items: pending, next })
    }
}

/// 已成功派发、已绑定 Sent、还差下一步的轮（判据见模块说明）。
async fn pending_rows(
    connection: &mut PgConnection,
    after: Option<CollectionCursor>,
    limit: i64,
) -> Result<Vec<PgRow>, WriteError> {
    let signed_cash = signed_versions(EvidenceCategory::CashClaim);
    Ok(sqlx::query(
        "SELECT attempt.contract_id, attempt.obligation_id, attempt.round, attempt.delivery_id,
                attempt.fence, attempt.attempt_id, attempt.started_at, obligation.obligation_kind,
                round_row.world_id AS send_world, binding.external_mail_id, send.sending_at,
                send.expectation, fulfillment.execution_state = 'satisfied' AS payment_satisfied
         FROM public.consignment_dispatch_attempts AS attempt
         JOIN public.consignment_dispatch_rounds AS round_row
           ON round_row.contract_id = attempt.contract_id
          AND round_row.obligation_id = attempt.obligation_id
          AND round_row.round = attempt.round
          AND round_row.delivery_id = attempt.delivery_id
         JOIN public.delivery_sent_bindings AS binding
           ON binding.delivery_id = attempt.delivery_id AND binding.fencing_version = attempt.fence
         JOIN public.delivery_send_attempts AS send
           ON send.delivery_id = attempt.delivery_id AND send.fencing_version = attempt.fence
         JOIN public.deliveries AS delivery
           ON delivery.id = attempt.delivery_id AND delivery.status = 'succeeded'
         JOIN public.settlement_obligations AS obligation
           ON obligation.id = attempt.obligation_id AND obligation.contract_id = attempt.contract_id
         JOIN public.settlement_obligation_fulfillments AS fulfillment
           ON fulfillment.obligation_id = obligation.id
         LEFT JOIN public.consignment_funded_sales AS funded
           ON funded.contract_id = attempt.contract_id
         WHERE ($3::timestamptz IS NULL
                OR (attempt.started_at, attempt.attempt_id) > ($3::timestamptz, $4::uuid))
           AND NOT EXISTS (
                   SELECT 1 FROM public.consignment_return_intakes AS intake
                   WHERE intake.contract_id = attempt.contract_id
                     AND intake.obligation_id = attempt.obligation_id
                     AND intake.original_round = attempt.round)
           AND CASE obligation.obligation_kind
               WHEN 'winner_equipment_cod' THEN
                   NOT EXISTS (SELECT 1 FROM public.consignment_handoff_proofs AS proof
                               WHERE proof.contract_id = attempt.contract_id
                                 AND proof.obligation_id = attempt.obligation_id)
                   AND COALESCE(funded.disposition, 'funded') = 'funded'
                   AND NOT EXISTS (SELECT 1 FROM public.consignment_sale_closures AS closure
                                   WHERE closure.contract_id = attempt.contract_id)
               WHEN 'winner_payment_carrier_cod' THEN
                   fulfillment.execution_state <> 'satisfied'
                   AND NOT EXISTS (SELECT 1 FROM public.consignment_sale_closures AS closure
                                   WHERE closure.contract_id = attempt.contract_id)
               WHEN 'winner_equipment_handoff' THEN
                   NOT EXISTS (SELECT 1 FROM public.consignment_handoff_proofs AS proof
                               WHERE proof.contract_id = attempt.contract_id
                                 AND proof.obligation_id = attempt.obligation_id)
                   AND funded.disposition = 'funded'
               WHEN 'seller_payout' THEN
                   NOT EXISTS (SELECT 1 FROM public.consignment_dispatch_evidence AS evidence
                               WHERE evidence.attempt_id = attempt.attempt_id
                                 AND evidence.category = 'cash_claim'
                                 AND evidence.validator_version = ANY ($1::text[]))
               WHEN 'buyer_refund' THEN
                   NOT EXISTS (SELECT 1 FROM public.consignment_dispatch_evidence AS evidence
                               WHERE evidence.attempt_id = attempt.attempt_id
                                 AND evidence.category = 'cash_claim'
                                 AND evidence.validator_version = ANY ($1::text[]))
               ELSE false END
         ORDER BY attempt.started_at, attempt.attempt_id
         LIMIT $2",
    )
    .bind(&signed_cash)
    .bind(limit)
    .bind(after.map(|cursor| cursor.started_at))
    .bind(after.map(|cursor| cursor.attempt_id))
    .fetch_all(connection)
    .await?)
}

/// 由待办行组装一项；义务种类不在闭集、冻结事实不全或已签收证据有歧义时返回 `None`（失败关闭，不猜测）。
async fn collection(
    connection: &mut PgConnection,
    row: &PgRow,
) -> Result<Option<PendingCollection>, WriteError> {
    let leg = match row.try_get::<&str, _>("obligation_kind")? {
        "winner_equipment_cod" => CollectionLeg::MainOriginalCod,
        "winner_payment_carrier_cod" => CollectionLeg::CarrierCod,
        "winner_equipment_handoff" => CollectionLeg::OriginalHandoff,
        "seller_payout" | "buyer_refund" => CollectionLeg::Cash,
        _ => return Ok(None),
    };
    let Some(send_world) = parse_settlement_world(row.try_get::<&str, _>("send_world")?) else {
        return Ok(None);
    };
    // 冻结 Send 身份由起写事务写入；形状不符说明事实被改坏，失败关闭而不是猜测观察目标。
    let expectation: DeliverySendExpectation = serde_json::from_value(row.try_get("expectation")?)
        .map_err(invariant_decoding("consignment_collection_expectation"))?;
    let attempt_id: Uuid = row.try_get("attempt_id")?;
    // 同一领取不应有两份已签收观察：多条即歧义，跳过该轮交人工核对。
    let evidence = match signed_evidence(connection, attempt_id, leg.category()).await?.as_slice() {
        [] => None,
        [evidence] => Some(evidence.clone()),
        _ => return Ok(None),
    };
    let key = DispatchRoundKey {
        contract_id: row.try_get("contract_id")?,
        obligation_id: row.try_get("obligation_id")?,
        round: u32::try_from(row.try_get::<i32, _>("round")?)?,
    };
    let attempt = AttemptKey {
        delivery_id: row.try_get("delivery_id")?,
        fence: u64::try_from(row.try_get::<i64, _>("fence")?)?,
    };
    let sent_mid: String = row.try_get("external_mail_id")?;
    let unpaid = matches!(leg, CollectionLeg::MainOriginalCod | CollectionLeg::CarrierCod)
        && !row.try_get::<bool, _>("payment_satisfied")?;
    let certify = if unpaid {
        let facts = certify_facts(connection, &key, attempt.delivery_id, &sent_mid, &expectation);
        match facts.await? {
            Some(facts) => Some(facts),
            None => return Ok(None),
        }
    } else {
        None
    };
    Ok(Some(PendingCollection {
        key,
        attempt,
        leg,
        send_world,
        sent_mid,
        sending_at: row.try_get("sending_at")?,
        expectation,
        evidence,
        certify,
    }))
}

/// 该尝试上出自已签收验证器的领取证据（至多读两条，供调用方判歧义）。
async fn signed_evidence(
    connection: &mut PgConnection,
    attempt_id: Uuid,
    category: EvidenceCategory,
) -> Result<Vec<CollectedEvidence>, WriteError> {
    let rows = sqlx::query(
        "SELECT remote_observation_id, digest FROM public.consignment_dispatch_evidence
         WHERE attempt_id = $1 AND category = $2 AND validator_version = ANY ($3::text[])
         ORDER BY remote_observation_id LIMIT 2",
    )
    .bind(attempt_id)
    .bind(category.as_str())
    .bind(signed_versions(category))
    .fetch_all(connection)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(CollectedEvidence {
                remote_observation_id: row.try_get("remote_observation_id")?,
                evidence_digest: digest(row, "digest")?,
            })
        })
        .collect()
}

/// 付款腿的认证事实：原派发的 prepare / activate 摘要、冻结正文摘要与 COD 毛额，成交冻结的价格、S / F、
/// 资金 UID 与模式，合同世界。activate 摘要缺失（投递未激活）即不可认证，返回 `None`。
async fn certify_facts(
    connection: &mut PgConnection,
    key: &DispatchRoundKey,
    delivery_id: Uuid,
    sent_mid: &str,
    expectation: &DeliverySendExpectation,
) -> Result<Option<CertifyCodFacts>, WriteError> {
    let row = sqlx::query(
        "SELECT delivery.request_digest, delivery.activation_request_digest,
                round_row.body_digest, round_row.cod, sale.price, sale.fee, sale.seller_amount,
                sale.funding_uid, sale.funding_mode, contract.world_id
         FROM public.consignment_dispatch_rounds AS round_row
         JOIN public.deliveries AS delivery ON delivery.id = round_row.delivery_id
         JOIN public.consignment_sales AS sale ON sale.contract_id = round_row.contract_id
         JOIN public.consignment_contracts AS contract ON contract.contract_id = round_row.contract_id
         WHERE round_row.delivery_id = $1 AND round_row.contract_id = $2",
    )
    .bind(delivery_id)
    .bind(key.contract_id)
    .fetch_one(connection)
    .await?;
    let Some(activation) = row.try_get::<Option<&[u8]>, _>("activation_request_digest")? else {
        return Ok(None);
    };
    let Some(world) = parse_settlement_world(row.try_get::<&str, _>("world_id")?) else {
        return Ok(None);
    };
    let mode = match row.try_get::<&str, _>("funding_mode")? {
        "strict_cash" => FundingMode::StrictCash,
        "record_only" => FundingMode::RecordOnly,
        _ => return Ok(None),
    };
    let price = ListingPrice::new(row.try_get("price")?)
        .map_err(invariant_decoding("consignment_collection_price"))?;
    Ok(Some(CertifyCodFacts {
        contract_id: key.contract_id,
        payment_obligation_id: key.obligation_id,
        original_delivery_id: delivery_id,
        receipt: ReceiptKey {
            uid: row.try_get("funding_uid")?,
            payment_mail_id: sent_mid.to_owned(),
            leg: CodLeg::for_world(world),
        },
        world,
        mode,
        price,
        quote: ListingQuote {
            fee: row.try_get("fee")?,
            seller_amount: row.try_get("seller_amount")?,
        },
        gross: row.try_get("cod")?,
        freeze: DispatchFreeze {
            prepare_digest: digest(&row, "request_digest")?,
            activation_digest: bytes32(activation)?,
            body_digest: digest(&row, "body_digest")?,
            equipment_ids: expectation.equipment_ids.clone(),
        },
    }))
}

fn digest(row: &PgRow, column: &'static str) -> Result<[u8; 32], WriteError> {
    bytes32(row.try_get::<&[u8], _>(column)?)
}

fn bytes32(bytes: &[u8]) -> Result<[u8; 32], WriteError> {
    <[u8; 32]>::try_from(bytes).map_err(invariant_decoding("consignment_collection_digest"))
}
