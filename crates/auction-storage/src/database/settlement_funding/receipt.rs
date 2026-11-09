//! 新COD来源的精确账号/邮件绑定；唯一协议观察来自A，金额来自原冻结业务。
//!
//! 业务准入在 `auction_application::settlement::CodReceiptSnapshot` 的纯判定中完成；本文件只负责按
//! 既有锁序读取、数据库时钟时效与最后约束查询，以及资金化失败的闭集拒绝码。

use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement_funding::account::LockedFundingAccount;
use crate::database::settlement_funding::{evidence_error, storage};
use auction_application::settlement::FundingError;
use auction_application::settlement::{
    CodReceiptBlocker, CodReceiptResultMetadata, CodReceiptSnapshot, FundingCollectedReceipt,
    evaluate_cod_receipt_evidence,
};
use auction_domain::SettlementLine;
use serde_json::json;
use sqlx::Postgres;
use sqlx::Row;
use uuid::Uuid;

mod diagnostics;
use diagnostics::{ReceiptRejection as Rejection, diagnose_relation, reject};

#[tracing::instrument(
    target = "auction::settlement::funding_diagnostics",
    level = "warn",
    skip_all,
    fields(obligation_id = %receipt.payment_obligation_id, delivery_id = %receipt.delivery_id)
)]
pub(super) async fn validate_receipt(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    receipt: &FundingCollectedReceipt<'_>,
    payment: Uuid,
    buyer: i64,
) -> Result<(), FundingError> {
    let Some(sent) = receipt.evidence.sent_collection.as_ref() else {
        return Err(reject(Rejection::SentMissing, FundingError::Unproven));
    };
    // 证据身份与账号绑定先于数据库读取；只细分日志，不放宽任何联合证明或身份条件。
    evaluate_cod_receipt_evidence(receipt, payment, buyer, account.uid(), account.records_only())
        .map_err(receipt_blocker_error)?;
    // generation/credential锁由调用者按统一前缀取得；此处只复核全快照实时时效，不倒锁。
    crate::validate_mail_evidence_time(tx, receipt.evidence)
        .await
        .map_err(receipt_evidence_error)?;
    // 关系完整性（义务/装备/履行/投递/命令与外部引用）仍由查询做最后约束；付款边界属于
    // 业务判定，改由 Rust 显式拒绝，使“投递早于资金初始化/对账”可分类定位。
    let row = sqlx::query(
        r#"SELECT o.obligation_kind,o.amount,o.carrier_credit,o.equipment_id,
        e.name AS equipment_name,d.payload,d.recipient,d.result_metadata,
        d.sending_at,
        (SELECT max(created_at) FROM public.settlement_funding_sources
          WHERE uid=$4 AND kind=$5) AS funding_source_boundary,
        CASE WHEN s.lot_id IS NULL THEN NULL ELSE jsonb_build_object(
          'lot_id',s.lot_id,'end_price',s.end_price,'listed_price',s.listed_price,
          'buyer_amount',s.buyer_amount,'seller_amount',s.seller_amount,
          'tax_amount',s.tax_amount,'compensation_amount',s.compensation_amount,
          'hold_ppm',s.hold_ppm,'adjustment_ppm',s.adjustment_ppm) END AS settlement_line,
        s.adjustment_metadata ->> 'policy' AS settlement_policy_mode,
        public.delivery_has_frozen_sent_proof(d.id,d.result_metadata,d.external_reference) AS frozen_sent_proven
        FROM public.settlement_obligations o
        JOIN public.equipment e ON e.id=o.equipment_id
        JOIN public.settlement_obligation_fulfillments f ON f.obligation_id=o.id
        JOIN public.deliveries d ON d.id=f.delivery_id
        LEFT JOIN public.lot_settlements s ON s.lot_id=o.lot_id
        WHERE o.id=$1 AND d.id=$2 AND f.execution_state='satisfied'
          AND f.proof_kind='mooglemail_sent_collected' AND f.proof_reference=$3
          AND f.proof_digest IS NOT NULL AND d.status='succeeded'
          AND d.origin_kind='settlement' AND d.origin_id=o.id::text
          AND d.external_reference=('mooglemail:main:send:' || $3)
          AND d.command_id=f.prepare_command_id AND d.request_digest=f.prepare_request_digest
          AND d.activation_command_id=f.activate_command_id
          AND d.activation_request_digest=f.activate_request_digest"#,
    )
    .bind(payment)
    .bind(receipt.delivery_id)
    .bind(&sent.external_mail_id)
    .bind(account.uid())
    .bind(if account.records_only() { "record_opening" } else { "reconciliation" })
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| reject(Rejection::QueryFailed, storage(error)))?;
    let Some(row) = row else {
        let kind = diagnose_relation(tx, account, receipt, payment, &sent.external_mail_id).await;
        return Err(reject(kind, FundingError::Unproven));
    };
    let snapshot = receipt_snapshot(&row)?;
    snapshot
        .evaluate_row(payment, &sent.recipient, &sent.subject, buyer)
        .map_err(receipt_blocker_error)
}

/// 把已锁定行投影为类型化快照；只有 SQL 类型或冻结明细解码失败才产生存储错误。
fn receipt_snapshot(row: &sqlx::postgres::PgRow) -> Result<CodReceiptSnapshot, FundingError> {
    let settlement_line: Option<serde_json::Value> =
        row.try_get("settlement_line").map_err(decode)?;
    let settlement_line = match settlement_line {
        Some(value) => Some(
            serde_json::from_value::<SettlementLine>(value)
                .map_err(|_| reject(Rejection::PricingRowDecodeFailed, FundingError::Unproven))?,
        ),
        None => None,
    };
    let metadata: serde_json::Value = row.try_get("result_metadata").map_err(decode)?;
    // 旧实现按 JSON 下标比较：缺键、`null` 或对象外形状都只会令证明条件为假。反序列化失败
    // 因此按“全部相关键缺失”处理，仍由 `SentProofUnproven` 失败关闭。
    let result_metadata: CodReceiptResultMetadata =
        serde_json::from_value(metadata).unwrap_or_default();
    Ok(CodReceiptSnapshot {
        obligation_kind: row.try_get("obligation_kind").map_err(decode)?,
        obligation_amount: row.try_get("amount").map_err(decode)?,
        carrier_credit: row.try_get("carrier_credit").map_err(decode)?,
        equipment_id: row.try_get("equipment_id").map_err(decode)?,
        equipment_name: row.try_get("equipment_name").map_err(decode)?,
        payload: row.try_get("payload").map_err(decode)?,
        recipient: row.try_get("recipient").map_err(decode)?,
        result_metadata,
        settlement_line,
        settlement_mode: row.try_get("settlement_policy_mode").map_err(decode)?,
        frozen_sent_proven: row.try_get("frozen_sent_proven").map_err(decode)?,
        sending_at: row.try_get("sending_at").map_err(decode)?,
        funding_source_boundary: row.try_get("funding_source_boundary").map_err(decode)?,
    })
}

/// 阻断项到既有拒绝码与资金错误的映射；只有 `CodOverflow` 保持 `Invalid`。
fn receipt_blocker_error(blocker: CodReceiptBlocker) -> FundingError {
    use CodReceiptBlocker as B;
    let (kind, error) = match blocker {
        B::SentMissing => (Rejection::SentMissing, FundingError::Unproven),
        B::SnapshotInvalid => (Rejection::SnapshotInvalid, FundingError::Unproven),
        B::PaymentMismatch => (Rejection::PaymentMismatch, FundingError::Unproven),
        B::BuyerMismatch => (Rejection::BuyerMismatch, FundingError::Unproven),
        B::SentUidMismatch => (Rejection::SentUidMismatch, FundingError::Unproven),
        B::SentWorldMismatch => (Rejection::SentWorldMismatch, FundingError::Unproven),
        B::CashUnexpected => (Rejection::CashUnexpected, FundingError::Unproven),
        B::CashMissing => (Rejection::CashMissing, FundingError::Unproven),
        B::CashWorldMismatch => (Rejection::CashWorldMismatch, FundingError::Unproven),
        B::CashUidMismatch => (Rejection::CashUidMismatch, FundingError::Unproven),
        B::MailIdInvalid => (Rejection::MailIdInvalid, FundingError::Unproven),
        B::OpeningSourceMissing => (Rejection::OpeningSourceMissing, FundingError::Unproven),
        B::SendingTimeMissing => (Rejection::SendingTimeMissing, FundingError::Unproven),
        B::SendBeforeFundingBoundary => (Rejection::SendBeforeBoundary, FundingError::Unproven),
        B::KindInvalid => (Rejection::KindInvalid, FundingError::Unproven),
        B::CarrierInvalid => (Rejection::CarrierInvalid, FundingError::Unproven),
        B::CodOverflow => (Rejection::CodOverflow, FundingError::Invalid),
        B::AmountMismatch => (Rejection::AmountMismatch, FundingError::Unproven),
        B::BuyerNonpositive => (Rejection::BuyerNonpositive, FundingError::Unproven),
        B::RecipientMismatch => (Rejection::RecipientMismatch, FundingError::Unproven),
        B::SubjectMismatch => (Rejection::SubjectMismatch, FundingError::Unproven),
        B::PricingEndPriceMissing => (Rejection::PricingEndPriceMissing, FundingError::Unproven),
        B::PricingLineInvalid => (Rejection::PricingLineInvalid, FundingError::Unproven),
        B::PricingBuyerMismatch => (Rejection::PricingBuyerMismatch, FundingError::Unproven),
        B::PricingHoldInvalid => (Rejection::PricingHoldInvalid, FundingError::Unproven),
        B::PricingModeInvalid => (Rejection::PricingModeInvalid, FundingError::Unproven),
        B::PricingPoolInvalid => (Rejection::PricingPoolInvalid, FundingError::Unproven),
        B::PricingAdjustedOverflow => (Rejection::PricingAdjustedOverflow, FundingError::Unproven),
        B::PayloadMismatch => (Rejection::PayloadMismatch, FundingError::Unproven),
        B::OperationMismatch => (Rejection::OperationMismatch, FundingError::Unproven),
        B::TemplateMismatch => (Rejection::TemplateMismatch, FundingError::Unproven),
        B::ReferenceUnproven => (Rejection::ReferenceUnproven, FundingError::Unproven),
        B::SentProofUnproven => (Rejection::SentProofUnproven, FundingError::Unproven),
    };
    reject(kind, error)
}

#[tracing::instrument(
    target = "auction::settlement::funding_diagnostics",
    level = "warn",
    skip_all,
    fields(obligation_id = %receipt.payment_obligation_id, delivery_id = %receipt.delivery_id)
)]
pub(super) async fn store_receipt(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    receipt: &FundingCollectedReceipt<'_>,
) -> Result<(), FundingError> {
    let sent = receipt
        .evidence
        .sent_collection
        .as_ref()
        .ok_or_else(|| reject(Rejection::StoreSentMissing, FundingError::Unproven))?;
    let cash = receipt.evidence.main_cash.as_ref();
    let evidence = json!({"credential_id":sent.binding.credential_id,
        "credential_version":sent.binding.credential_version,"runtime_generation":sent.binding.runtime.generation,
        "uid":sent.binding.authenticated_uid,"player_name":sent.binding.player_name,
        "protocol":sent.binding.protocol_version,"buyer_credits":receipt.buyer_credits,
        "recipient":sent.recipient,"subject":sent.subject,"sent_at":sent.sent_at,
        "collected":sent.collected,"remaining_credits":sent.remaining_credits,"cod_pending":sent.cod_pending,
        "remaining_equipment_ids":sent.remaining_equipment_ids,
        "receipt_observed_at":sent.observed_at,"receipt_digest":sent.digest,
        "binding_digest":sent.binding.digest,"cash_credits":cash.map(|cash|cash.credits),
        "cash_observed_at":cash.map(|cash|cash.observed_at),"cash_digest":cash.map(|cash|cash.digest),"snapshot_digest":receipt.evidence.digest});
    sqlx::query("INSERT INTO public.settlement_funding_receipts(source_id,uid,payment_obligation_id,delivery_id,external_mail_id,evidence) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(source).bind(&sent.binding.authenticated_uid).bind(receipt.payment_obligation_id)
        .bind(receipt.delivery_id).bind(&sent.external_mail_id).bind(evidence)
        .execute(&mut **tx).await.map_err(|error| reject(Rejection::StoreFailed, storage(error)))?;
    Ok(())
}

fn decode(error: sqlx::Error) -> FundingError {
    reject(Rejection::RowDecodeFailed, storage(error))
}

fn receipt_evidence_error(error: crate::MailEvidenceTransactionError) -> FundingError {
    let kind = match &error {
        crate::MailEvidenceTransactionError::Sqlx(_) => Rejection::QueryFailed,
        crate::MailEvidenceTransactionError::Rejected(_) => Rejection::EvidenceTimeInvalid,
    };
    reject(kind, evidence_error(error))
}
