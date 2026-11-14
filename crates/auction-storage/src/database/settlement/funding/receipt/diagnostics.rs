//! 收据拒绝的唯一内部字典与只读辅助诊断；原授权 SQL 和原错误决定业务结果。

use auction_application::settlement::{FundingCollectedReceipt, FundingError};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::super::{LockedFundingAccount, diagnostics};
use crate::database::TrackedTransaction as Transaction;

/// 收据与价格拒绝码的唯一内部字典；只用于日志，绝不写入持久化失败码或替代资金授权。
/// 每个变体的稳定 `safe_kind` 由 [`ReceiptRejection::as_str`] 给出。
#[derive(Clone, Copy)]
pub(super) enum ReceiptRejection {
    /// 联合证据缺少 Sent。
    SentMissing,
    /// 联合快照结构或摘要校验不通过。
    SnapshotInvalid,
    /// 收据付款义务不等于冻结付款。
    PaymentMismatch,
    /// 收据买方金额不等于冻结金额。
    BuyerMismatch,
    /// Sent 与已锁资金账户身份不等。
    SentUidMismatch,
    /// Sent 世界不是 main。
    SentWorldMismatch,
    /// record_only 却携带现金证据。
    CashUnexpected,
    /// strict_cash 缺少现金证据。
    CashMissing,
    /// 现金证据世界不是 main。
    CashWorldMismatch,
    /// 现金证据与资金账户身份不等。
    CashUidMismatch,
    /// 邮件标识不是规范正整数。
    MailIdInvalid,
    /// 数据库时间下证据未获证明。
    EvidenceTimeInvalid,
    /// 授权查询无行，诊断关闭、失败、超时、预算不足或未能进一步分类。
    RelationUnproven,
    /// 原收据 SQL 执行失败，原因只保留资金错误分类。
    QueryFailed,
    /// 原收据行解码失败。
    RowDecodeFailed,
    /// 义务种类不是两种 COD。
    KindInvalid,
    /// COD 种类对应的 carrier 不是 0 或 1。
    CarrierInvalid,
    /// 买方金额加 carrier 溢出，原返回 Invalid。
    CodOverflow,
    /// 义务金额与冻结买方金额不等。
    AmountMismatch,
    /// 买方金额非正。
    BuyerNonpositive,
    /// Sent 与原投递收件人不等。
    RecipientMismatch,
    /// Sent 主题与 canonical 主题不等。
    SubjectMismatch,
    /// 原 payload 不等于完整重建值。
    PayloadMismatch,
    /// metadata 操作不是 send。
    OperationMismatch,
    /// metadata 模板不符。
    TemplateMismatch,
    /// metadata 外部引用未证明。
    ReferenceUnproven,
    /// 旧精确证明与冻结强证明均不成立。
    SentProofUnproven,
    /// 持久收据前缺少 Sent。
    StoreSentMissing,
    /// 原收据 INSERT 失败。
    StoreFailed,
    /// 授权关联的付款义务缺失。
    ObligationMissing,
    /// 授权关联的装备缺失。
    EquipmentMissing,
    /// 授权关联的履约缺失。
    FulfillmentMissing,
    /// 授权关联的投递缺失。
    DeliveryMissing,
    /// 履约关联投递与收据投递不等。
    DeliveryMismatch,
    /// 履约尚非 satisfied。
    FulfillmentStateMismatch,
    /// 履约证明种类不符。
    ProofKindMismatch,
    /// 履约证明引用不符。
    ProofReferenceMismatch,
    /// 履约证明摘要缺失。
    ProofDigestMissing,
    /// 投递尚非 succeeded。
    DeliveryStateMismatch,
    /// 投递 origin kind 不是 settlement。
    OriginKindMismatch,
    /// 投递 origin id 与付款义务不等。
    OriginIdMismatch,
    /// 投递引用不符。
    ExternalReferenceMismatch,
    /// prepare 命令不等。
    PrepareCommandMismatch,
    /// prepare 摘要不等。
    PrepareDigestMismatch,
    /// activate 命令不等。
    ActivateCommandMismatch,
    /// activate 摘要不等。
    ActivateDigestMismatch,
    /// record_opening 或 reconciliation 边界来源缺失。
    OpeningSourceMissing,
    /// 原 sending_at 缺失。
    SendingTimeMissing,
    /// 原 sending_at 未严格晚于资金边界。
    SendBeforeBoundary,
    /// 冻结 end_price 缺失。
    PricingEndPriceMissing,
    /// 冻结价格行解码失败。
    PricingRowDecodeFailed,
    /// 冻结买方奖池金额无效。
    PricingPoolInvalid,
    /// adjusted price 加法溢出。
    PricingAdjustedOverflow,
    /// 冻结 buyer amount 与本次 buyer 不等。
    PricingBuyerMismatch,
    /// 冻结 hold_ppm 越界。
    PricingHoldInvalid,
    /// 冻结策略模式不能解码。
    PricingModeInvalid,
    /// 冻结结算明细不符合冻结策略。
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
            Self::PricingRowDecodeFailed => "funding_pricing_row_decode_failed",
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
            | Self::PricingRowDecodeFailed
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
