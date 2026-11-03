//! 收据拒绝的唯一内部字典与只读辅助诊断；原授权 SQL 和原错误决定业务结果。

use super::super::{LockedFundingAccount, diagnostics};
use crate::database::TrackedTransaction as Transaction;
use auction_application::{FundingCollectedReceipt, FundingError};
use sqlx::{Postgres, Row};
use uuid::Uuid;

/// 只用于日志，绝不写入持久化失败码或替代资金授权。
#[derive(Clone, Copy)]
pub(super) enum ReceiptRejection {
    SentMissing,
    SnapshotInvalid,
    PaymentMismatch,
    BuyerMismatch,
    SentUidMismatch,
    SentWorldMismatch,
    CashUnexpected,
    CashMissing,
    CashWorldMismatch,
    CashUidMismatch,
    MailIdInvalid,
    EvidenceTimeInvalid,
    RelationUnproven,
    QueryFailed,
    RowDecodeFailed,
    KindInvalid,
    CarrierInvalid,
    CodOverflow,
    AmountMismatch,
    BuyerNonpositive,
    RecipientMismatch,
    SubjectMismatch,
    PayloadMismatch,
    OperationMismatch,
    TemplateMismatch,
    ReferenceUnproven,
    SentProofUnproven,
    StoreSentMissing,
    StoreFailed,
    ObligationMissing,
    EquipmentMissing,
    FulfillmentMissing,
    DeliveryMissing,
    DeliveryMismatch,
    FulfillmentStateMismatch,
    ProofKindMismatch,
    ProofReferenceMismatch,
    ProofDigestMissing,
    DeliveryStateMismatch,
    OriginKindMismatch,
    OriginIdMismatch,
    ExternalReferenceMismatch,
    PrepareCommandMismatch,
    PrepareDigestMismatch,
    ActivateCommandMismatch,
    ActivateDigestMismatch,
    OpeningSourceMissing,
    SendingTimeMissing,
    SendBeforeBoundary,
    PricingEndPriceMissing,
    PricingEndPriceInvalid,
    PricingListedPriceMissing,
    PricingListedPriceInvalid,
    PricingBuyerAmountMissing,
    PricingBuyerAmountInvalid,
    PricingSellerAmountMissing,
    PricingSellerAmountInvalid,
    PricingTaxAmountMissing,
    PricingTaxAmountInvalid,
    PricingCompensationAmountMissing,
    PricingCompensationAmountInvalid,
    PricingRowDecodeFailed,
    PricingHoldMissing,
    PricingAdjustmentMissing,
    PricingPoolInvalid,
    PricingAdjustedOverflow,
    PricingBuyerMismatch,
    PricingHoldInvalid,
    PricingModeInvalid,
    PricingLineInvalid,
}

impl ReceiptRejection {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::SentMissing => "funding_receipt_sent_missing",
            Self::SnapshotInvalid => "funding_receipt_snapshot_invalid",
            Self::PaymentMismatch => "funding_receipt_payment_mismatch",
            Self::BuyerMismatch => "funding_receipt_buyer_mismatch",
            Self::SentUidMismatch => "funding_receipt_sent_uid_mismatch",
            Self::SentWorldMismatch => "funding_receipt_sent_world_mismatch",
            Self::CashUnexpected => "funding_receipt_cash_unexpected",
            Self::CashMissing => "funding_receipt_cash_missing",
            Self::CashWorldMismatch => "funding_receipt_cash_world_mismatch",
            Self::CashUidMismatch => "funding_receipt_cash_uid_mismatch",
            Self::MailIdInvalid => "funding_receipt_mail_id_invalid",
            Self::EvidenceTimeInvalid => "funding_receipt_evidence_time_invalid",
            Self::RelationUnproven => "funding_receipt_relation_unproven",
            Self::QueryFailed => "funding_receipt_query_failed",
            Self::RowDecodeFailed => "funding_receipt_row_decode_failed",
            Self::KindInvalid => "funding_receipt_kind_invalid",
            Self::CarrierInvalid => "funding_receipt_carrier_invalid",
            Self::CodOverflow => "funding_receipt_cod_overflow",
            Self::AmountMismatch => "funding_receipt_amount_mismatch",
            Self::BuyerNonpositive => "funding_receipt_buyer_nonpositive",
            Self::RecipientMismatch => "funding_receipt_recipient_mismatch",
            Self::SubjectMismatch => "funding_receipt_subject_mismatch",
            Self::PayloadMismatch => "funding_receipt_payload_mismatch",
            Self::OperationMismatch => "funding_receipt_operation_mismatch",
            Self::TemplateMismatch => "funding_receipt_template_mismatch",
            Self::ReferenceUnproven => "funding_receipt_reference_unproven",
            Self::SentProofUnproven => "funding_receipt_sent_proof_unproven",
            Self::StoreSentMissing => "funding_receipt_store_sent_missing",
            Self::StoreFailed => "funding_receipt_store_failed",
            Self::ObligationMissing => "funding_receipt_obligation_missing",
            Self::EquipmentMissing => "funding_receipt_equipment_missing",
            Self::FulfillmentMissing => "funding_receipt_fulfillment_missing",
            Self::DeliveryMissing => "funding_receipt_delivery_missing",
            Self::DeliveryMismatch => "funding_receipt_delivery_mismatch",
            Self::FulfillmentStateMismatch => "funding_receipt_fulfillment_state_mismatch",
            Self::ProofKindMismatch => "funding_receipt_proof_kind_mismatch",
            Self::ProofReferenceMismatch => "funding_receipt_proof_reference_mismatch",
            Self::ProofDigestMissing => "funding_receipt_proof_digest_missing",
            Self::DeliveryStateMismatch => "funding_receipt_delivery_state_mismatch",
            Self::OriginKindMismatch => "funding_receipt_origin_kind_mismatch",
            Self::OriginIdMismatch => "funding_receipt_origin_id_mismatch",
            Self::ExternalReferenceMismatch => "funding_receipt_external_reference_mismatch",
            Self::PrepareCommandMismatch => "funding_receipt_prepare_command_mismatch",
            Self::PrepareDigestMismatch => "funding_receipt_prepare_digest_mismatch",
            Self::ActivateCommandMismatch => "funding_receipt_activate_command_mismatch",
            Self::ActivateDigestMismatch => "funding_receipt_activate_digest_mismatch",
            Self::OpeningSourceMissing => "funding_receipt_opening_source_missing",
            Self::SendingTimeMissing => "funding_receipt_sending_time_missing",
            Self::SendBeforeBoundary => "funding_receipt_send_before_boundary",
            Self::PricingEndPriceMissing => "funding_pricing_end_price_missing",
            Self::PricingEndPriceInvalid => "funding_pricing_end_price_invalid",
            Self::PricingListedPriceMissing => "funding_pricing_listed_price_missing",
            Self::PricingListedPriceInvalid => "funding_pricing_listed_price_invalid",
            Self::PricingBuyerAmountMissing => "funding_pricing_buyer_amount_missing",
            Self::PricingBuyerAmountInvalid => "funding_pricing_buyer_amount_invalid",
            Self::PricingSellerAmountMissing => "funding_pricing_seller_amount_missing",
            Self::PricingSellerAmountInvalid => "funding_pricing_seller_amount_invalid",
            Self::PricingTaxAmountMissing => "funding_pricing_tax_amount_missing",
            Self::PricingTaxAmountInvalid => "funding_pricing_tax_amount_invalid",
            Self::PricingCompensationAmountMissing => "funding_pricing_compensation_amount_missing",
            Self::PricingCompensationAmountInvalid => "funding_pricing_compensation_amount_invalid",
            Self::PricingRowDecodeFailed => "funding_pricing_row_decode_failed",
            Self::PricingHoldMissing => "funding_pricing_hold_missing",
            Self::PricingAdjustmentMissing => "funding_pricing_adjustment_missing",
            Self::PricingPoolInvalid => "funding_pricing_pool_invalid",
            Self::PricingAdjustedOverflow => "funding_pricing_adjusted_overflow",
            Self::PricingBuyerMismatch => "funding_pricing_buyer_mismatch",
            Self::PricingHoldInvalid => "funding_pricing_hold_invalid",
            Self::PricingModeInvalid => "funding_pricing_mode_invalid",
            Self::PricingLineInvalid => "funding_pricing_line_invalid",
        }
    }

    fn stage(self) -> &'static str {
        match self {
            Self::PricingEndPriceMissing
            | Self::PricingEndPriceInvalid
            | Self::PricingListedPriceMissing
            | Self::PricingListedPriceInvalid
            | Self::PricingBuyerAmountMissing
            | Self::PricingBuyerAmountInvalid
            | Self::PricingSellerAmountMissing
            | Self::PricingSellerAmountInvalid
            | Self::PricingTaxAmountMissing
            | Self::PricingTaxAmountInvalid
            | Self::PricingCompensationAmountMissing
            | Self::PricingCompensationAmountInvalid
            | Self::PricingRowDecodeFailed
            | Self::PricingHoldMissing
            | Self::PricingAdjustmentMissing
            | Self::PricingPoolInvalid
            | Self::PricingAdjustedOverflow
            | Self::PricingBuyerMismatch
            | Self::PricingHoldInvalid
            | Self::PricingModeInvalid
            | Self::PricingLineInvalid => "receipt_pricing",
            Self::StoreSentMissing | Self::StoreFailed => "receipt_store",
            Self::RelationUnproven
            | Self::ObligationMissing
            | Self::EquipmentMissing
            | Self::FulfillmentMissing
            | Self::DeliveryMissing
            | Self::DeliveryMismatch
            | Self::FulfillmentStateMismatch
            | Self::ProofKindMismatch
            | Self::ProofReferenceMismatch
            | Self::ProofDigestMissing
            | Self::DeliveryStateMismatch
            | Self::OriginKindMismatch
            | Self::OriginIdMismatch
            | Self::ExternalReferenceMismatch
            | Self::PrepareCommandMismatch
            | Self::PrepareDigestMismatch
            | Self::ActivateCommandMismatch
            | Self::ActivateDigestMismatch
            | Self::OpeningSourceMissing
            | Self::SendingTimeMissing
            | Self::SendBeforeBoundary => "receipt_relation",
            _ => "receipt_validate",
        }
    }
}

/// 所有收据拒绝都经此 helper 留下安全类别，再返回完全相同的原资金错误。
pub(super) fn reject(kind: ReceiptRejection, error: FundingError) -> FundingError {
    diagnostics::reject(kind.as_str(), kind.stage(), error)
}

/// 仅在原授权查询无行后读取布尔谓词；失败仍是原 Unproven，不授予任何通过资格。
pub(super) async fn diagnose_relation(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    receipt: &FundingCollectedReceipt<'_>,
    payment: Uuid,
    external_mail_id: &str,
) -> ReceiptRejection {
    if !tracing::enabled!(target: diagnostics::TARGET, tracing::Level::WARN) {
        return ReceiptRejection::RelationUnproven;
    }
    // 原授权已经拒绝；余量不足一秒时不再诊断，给原回滚路径保留预算，不刷新 transaction_timeout。
    if receipt.evidence.expires_at - chrono::Utc::now() < chrono::Duration::seconds(1) {
        return ReceiptRejection::RelationUnproven;
    }
    // 不在客户端取消执行中的语句，避免 sqlx 在后续 rollback 时等待并排空剩余响应。
    // SET 或 SELECT 的 SQL 错误（包括 57014）只使事务中止，不妨碍原 ROLLBACK；仍返回原 Unproven。
    // 本分支必走原回滚，LOCAL 设置也随之撤销，不影响连接后续的其他事务。
    if sqlx::query("SET LOCAL statement_timeout = '250ms'").execute(&mut **tx).await.is_err() {
        return ReceiptRejection::RelationUnproven;
    }
    let query = sqlx::query(
        r#"SELECT
        o.id IS NOT NULL AS obligation_exists,
        e.id IS NOT NULL AS equipment_exists,
        f.obligation_id IS NOT NULL AS fulfillment_exists,
        d.id IS NOT NULL AS delivery_exists,
        d.id=$2 AS delivery_matches,
        f.execution_state='satisfied' AS fulfillment_state_matches,
        f.proof_kind='mooglemail_sent_collected' AS proof_kind_matches,
        f.proof_reference=$3 AS proof_reference_matches,
        f.proof_digest IS NOT NULL AS proof_digest_exists,
        d.status='succeeded' AS delivery_state_matches,
        d.origin_kind='settlement' AS origin_kind_matches,
        d.origin_id=o.id::text AS origin_id_matches,
        d.external_reference=('mooglemail:main:send:' || $3) AS external_reference_matches,
        d.command_id=f.prepare_command_id AS prepare_command_matches,
        d.request_digest=f.prepare_request_digest AS prepare_digest_matches,
        d.activation_command_id=f.activate_command_id AS activate_command_matches,
        d.activation_request_digest=f.activate_request_digest AS activate_digest_matches,
        boundary.created_at IS NOT NULL AS opening_source_exists,
        d.sending_at IS NOT NULL AS sending_time_exists,
        d.sending_at > boundary.created_at AS sending_after_boundary
        FROM (SELECT $1::uuid AS obligation_id) requested
        LEFT JOIN public.settlement_obligations o ON o.id=requested.obligation_id
        LEFT JOIN public.equipment e ON e.id=o.equipment_id
        LEFT JOIN public.settlement_obligation_fulfillments f ON f.obligation_id=o.id
        LEFT JOIN public.deliveries d ON d.id=f.delivery_id
        LEFT JOIN LATERAL (
            SELECT max(created_at) AS created_at FROM public.settlement_funding_sources
            WHERE uid=$4 AND kind=$5
        ) boundary ON true"#,
    )
    .bind(payment)
    .bind(receipt.delivery_id)
    .bind(external_mail_id)
    .bind(account.uid())
    .bind(if account.records_only() { "record_opening" } else { "reconciliation" });
    // 完整等待服务端结果；不 spawn/detach，诊断或行解码失败都不能替换原拒绝。
    let row = match query.fetch_optional(&mut **tx).await {
        Ok(Some(row)) => row,
        Ok(None) | Err(_) => return ReceiptRejection::RelationUnproven,
    };
    for (kind, column) in [
        (ReceiptRejection::ObligationMissing, "obligation_exists"),
        (ReceiptRejection::EquipmentMissing, "equipment_exists"),
        (ReceiptRejection::FulfillmentMissing, "fulfillment_exists"),
        (ReceiptRejection::DeliveryMissing, "delivery_exists"),
        (ReceiptRejection::DeliveryMismatch, "delivery_matches"),
        (ReceiptRejection::FulfillmentStateMismatch, "fulfillment_state_matches"),
        (ReceiptRejection::ProofKindMismatch, "proof_kind_matches"),
        (ReceiptRejection::ProofReferenceMismatch, "proof_reference_matches"),
        (ReceiptRejection::ProofDigestMissing, "proof_digest_exists"),
        (ReceiptRejection::DeliveryStateMismatch, "delivery_state_matches"),
        (ReceiptRejection::OriginKindMismatch, "origin_kind_matches"),
        (ReceiptRejection::OriginIdMismatch, "origin_id_matches"),
        (ReceiptRejection::ExternalReferenceMismatch, "external_reference_matches"),
        (ReceiptRejection::PrepareCommandMismatch, "prepare_command_matches"),
        (ReceiptRejection::PrepareDigestMismatch, "prepare_digest_matches"),
        (ReceiptRejection::ActivateCommandMismatch, "activate_command_matches"),
        (ReceiptRejection::ActivateDigestMismatch, "activate_digest_matches"),
        (ReceiptRejection::OpeningSourceMissing, "opening_source_exists"),
        (ReceiptRejection::SendingTimeMissing, "sending_time_exists"),
        (ReceiptRejection::SendBeforeBoundary, "sending_after_boundary"),
    ] {
        match row.try_get::<Option<bool>, _>(column) {
            Ok(Some(true)) => {}
            Ok(Some(false) | None) => return kind,
            Err(_) => return ReceiptRejection::RelationUnproven,
        }
    }
    ReceiptRejection::RelationUnproven
}
