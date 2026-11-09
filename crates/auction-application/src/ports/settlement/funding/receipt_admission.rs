// COD 实收资金化准入：同账号 Sent 收据、冻结业务金额与付款边界。
//
// 存储层按既有锁序读取义务、履行、投递与结算明细，投影成本结构；本模块只做纯判定。
// 数据库查询仍是不可绕过的最后约束（关系完整性、已绑定投递与证明列），但业务判定以本
// 模块为准：义务种类与载体配对、金额、收件人、主题、载荷、proof 标记与付款边界。
//
// 旧 SQL 直接在 `sending_at > 付款来源建立时间` 上过滤，越界时只剩“无行”并依赖
// 事后诊断。现在边界作为显式阻断项返回，日志保持同一拒绝码，便于按 `safe_kind`/`stage`
// 直接定位“投递早于资金初始化/对账”的原因。

use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use auction_domain::{
    SETTLEMENT_PPM, SettlementId, SettlementLine, SettlementMode, SettlementObligationKind,
    SettlementWorld,
};

use crate::ports::settlement::FundingCollectedReceipt;

/// 冻结载荷变量；缺省字段必须保持 `None` 才能与旧载荷逐字段相等。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodReceiptVariables {
    pub reference: String,
    pub amount: String,
    #[serde(default)]
    pub item_name: Option<String>,
    #[serde(default)]
    pub pricing_mode: Option<String>,
    #[serde(default)]
    pub hold_ppm: Option<String>,
    #[serde(default)]
    pub adjustment_ppm: Option<String>,
    #[serde(default)]
    pub winning_bid: Option<String>,
    #[serde(default)]
    pub adjusted_price: Option<String>,
    #[serde(default)]
    pub pool_credit: Option<String>,
}

/// 冻结载荷中的装备 secret 引用。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodReceiptSecretRef {
    pub kind: String,
    pub id: String,
}

/// 版本化发送载荷；未知键、缺键或类型变化都视为载荷不符。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodReceiptPayload {
    pub schema_version: u16,
    pub operation: String,
    pub world: String,
    pub template: String,
    pub variables: CodReceiptVariables,
    pub credits: i64,
    pub cod: i64,
    pub equipment_secret_refs: Vec<CodReceiptSecretRef>,
}

/// 投递结果元数据中与资金化相关的键；其余键不参与判定。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct CodReceiptResultMetadata {
    pub proof: Option<String>,
    pub sent_verified: Option<bool>,
    pub settlement_obligation_id: Option<String>,
    pub operation: Option<String>,
    pub template: Option<String>,
    pub external_reference_proven: Option<bool>,
}

/// 已锁定的 COD 收据行投影；金额与明细来自冻结结算事实，不信任载荷自报数值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodReceiptSnapshot {
    pub obligation_kind: String,
    pub obligation_amount: i64,
    pub carrier_credit: i64,
    pub equipment_id: Uuid,
    pub equipment_name: String,
    /// 投递载荷本身是冻结的版本化 JSON 契约；业务比较先反序列化为 [`CodReceiptPayload`]。
    pub payload: serde_json::Value,
    pub recipient: String,
    pub result_metadata: CodReceiptResultMetadata,
    pub settlement_line: Option<SettlementLine>,
    pub settlement_mode: Option<String>,
    pub frozen_sent_proven: bool,
    pub sending_at: Option<DateTime<Utc>>,
    pub funding_source_boundary: Option<DateTime<Utc>>,
}

/// 阻断项闭集；顺序即拒绝优先级，调用方取第一项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodReceiptBlocker {
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
    OpeningSourceMissing,
    SendingTimeMissing,
    SendBeforeFundingBoundary,
    KindInvalid,
    CarrierInvalid,
    CodOverflow,
    AmountMismatch,
    BuyerNonpositive,
    RecipientMismatch,
    SubjectMismatch,
    PricingEndPriceMissing,
    PricingLineInvalid,
    PricingBuyerMismatch,
    PricingHoldInvalid,
    PricingModeInvalid,
    PricingPoolInvalid,
    PricingAdjustedOverflow,
    PayloadMismatch,
    OperationMismatch,
    TemplateMismatch,
    ReferenceUnproven,
    SentProofUnproven,
}

impl CodReceiptBlocker {
    /// 稳定标识；只用于测试与日志分类，改写前须同步既有 `funding_receipt_*` 字典。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SentMissing => "sent_missing",
            Self::SnapshotInvalid => "snapshot_invalid",
            Self::PaymentMismatch => "payment_mismatch",
            Self::BuyerMismatch => "buyer_mismatch",
            Self::SentUidMismatch => "sent_uid_mismatch",
            Self::SentWorldMismatch => "sent_world_mismatch",
            Self::CashUnexpected => "cash_unexpected",
            Self::CashMissing => "cash_missing",
            Self::CashWorldMismatch => "cash_world_mismatch",
            Self::CashUidMismatch => "cash_uid_mismatch",
            Self::MailIdInvalid => "mail_id_invalid",
            Self::OpeningSourceMissing => "opening_source_missing",
            Self::SendingTimeMissing => "sending_time_missing",
            Self::SendBeforeFundingBoundary => "send_before_funding_boundary",
            Self::KindInvalid => "kind_invalid",
            Self::CarrierInvalid => "carrier_invalid",
            Self::CodOverflow => "cod_overflow",
            Self::AmountMismatch => "amount_mismatch",
            Self::BuyerNonpositive => "buyer_nonpositive",
            Self::RecipientMismatch => "recipient_mismatch",
            Self::SubjectMismatch => "subject_mismatch",
            Self::PricingEndPriceMissing => "pricing_end_price_missing",
            Self::PricingLineInvalid => "pricing_line_invalid",
            Self::PricingBuyerMismatch => "pricing_buyer_mismatch",
            Self::PricingHoldInvalid => "pricing_hold_invalid",
            Self::PricingModeInvalid => "pricing_mode_invalid",
            Self::PricingPoolInvalid => "pricing_pool_invalid",
            Self::PricingAdjustedOverflow => "pricing_adjusted_overflow",
            Self::PayloadMismatch => "payload_mismatch",
            Self::OperationMismatch => "operation_mismatch",
            Self::TemplateMismatch => "template_mismatch",
            Self::ReferenceUnproven => "reference_unproven",
            Self::SentProofUnproven => "sent_proof_unproven",
        }
    }
}

/// 期望载荷；与冻结载荷逐字段比较，不做宽松归一化。
struct ExpectedPayload {
    template: &'static str,
    variables: CodReceiptVariables,
    credits: i64,
    cod: i64,
    equipment_secret_refs: Vec<CodReceiptSecretRef>,
}

/// 证据身份与账号绑定判定；数据库时钟时效另由存储层在同一事务内复核。
pub fn evaluate_cod_receipt_evidence(
    receipt: &FundingCollectedReceipt<'_>,
    payment: Uuid,
    buyer: i64,
    account_uid: &str,
    records_only: bool,
) -> Result<(), CodReceiptBlocker> {
    let evidence = receipt.evidence;
    let Some(sent) = evidence.sent_collection.as_ref() else {
        return Err(CodReceiptBlocker::SentMissing);
    };
    if !evidence.validate() {
        return Err(CodReceiptBlocker::SnapshotInvalid);
    }
    if receipt.payment_obligation_id != payment {
        return Err(CodReceiptBlocker::PaymentMismatch);
    }
    if receipt.buyer_credits != buyer {
        return Err(CodReceiptBlocker::BuyerMismatch);
    }
    if sent.binding.authenticated_uid != account_uid {
        return Err(CodReceiptBlocker::SentUidMismatch);
    }
    if sent.binding.world != SettlementWorld::Main {
        return Err(CodReceiptBlocker::SentWorldMismatch);
    }
    let cash = evidence.main_cash.as_ref();
    if records_only {
        if cash.is_some() {
            return Err(CodReceiptBlocker::CashUnexpected);
        }
    } else {
        let cash = cash.ok_or(CodReceiptBlocker::CashMissing)?;
        if cash.binding.world != SettlementWorld::Main {
            return Err(CodReceiptBlocker::CashWorldMismatch);
        }
        if cash.binding.authenticated_uid != account_uid {
            return Err(CodReceiptBlocker::CashUidMismatch);
        }
    }
    if !crate::ports::settlement::valid_external_positive_id(&sent.external_mail_id) {
        return Err(CodReceiptBlocker::MailIdInvalid);
    }
    Ok(())
}

impl CodReceiptSnapshot {
    /// 完整纯判定：先证据身份，再行内业务事实。
    pub fn evaluate(
        &self,
        receipt: &FundingCollectedReceipt<'_>,
        payment: Uuid,
        buyer: i64,
        account_uid: &str,
        records_only: bool,
    ) -> Result<(), CodReceiptBlocker> {
        let Some(sent) = receipt.evidence.sent_collection.as_ref() else {
            return Err(CodReceiptBlocker::SentMissing);
        };
        evaluate_cod_receipt_evidence(receipt, payment, buyer, account_uid, records_only)?;
        self.evaluate_row(payment, &sent.recipient, &sent.subject, buyer)
    }

    /// 行内业务判定；调用方须已通过证据身份与数据库时钟时效校验。
    pub fn evaluate_row(
        &self,
        payment: Uuid,
        sent_recipient: &str,
        sent_subject: &str,
        buyer: i64,
    ) -> Result<(), CodReceiptBlocker> {
        let sending_at = self.sending_at.ok_or(CodReceiptBlocker::SendingTimeMissing)?;
        let boundary =
            self.funding_source_boundary.ok_or(CodReceiptBlocker::OpeningSourceMissing)?;
        if sending_at <= boundary {
            return Err(CodReceiptBlocker::SendBeforeFundingBoundary);
        }
        let (kind, template) = match self.obligation_kind.as_str() {
            "winner_equipment_cod" if self.carrier_credit == 0 => {
                (SettlementObligationKind::WinnerEquipmentCod, "settlement_winner_equipment_cod")
            }
            "winner_payment_carrier_cod" if self.carrier_credit == 1 => {
                (SettlementObligationKind::WinnerPaymentCarrierCod, "settlement_payment_carrier")
            }
            "winner_equipment_cod" | "winner_payment_carrier_cod" => {
                return Err(CodReceiptBlocker::CarrierInvalid);
            }
            _ => return Err(CodReceiptBlocker::KindInvalid),
        };
        let cod = buyer.checked_add(self.carrier_credit).ok_or(CodReceiptBlocker::CodOverflow)?;
        if self.obligation_amount != buyer {
            return Err(CodReceiptBlocker::AmountMismatch);
        }
        if buyer <= 0 {
            return Err(CodReceiptBlocker::BuyerNonpositive);
        }
        if self.recipient != sent_recipient {
            return Err(CodReceiptBlocker::RecipientMismatch);
        }
        if crate::ports::settlement::canonical_settlement_fulfillment_subject(
            kind,
            SettlementId::from_uuid(payment),
        )
        .as_deref()
            != Some(sent_subject)
        {
            return Err(CodReceiptBlocker::SubjectMismatch);
        }
        let expected = self.expected_payload(template, payment, buyer, cod)?;
        let payload: CodReceiptPayload = serde_json::from_value(self.payload.clone())
            .ok()
            .ok_or(CodReceiptBlocker::PayloadMismatch)?;
        if payload.schema_version != 1
            || payload.operation != "send"
            || payload.world != "main"
            || payload.template != expected.template
            || payload.credits != expected.credits
            || payload.cod != expected.cod
            || payload.variables != expected.variables
            || payload.equipment_secret_refs != expected.equipment_secret_refs
        {
            return Err(CodReceiptBlocker::PayloadMismatch);
        }
        let metadata = &self.result_metadata;
        if metadata.operation.as_deref() != Some("send") {
            return Err(CodReceiptBlocker::OperationMismatch);
        }
        if metadata.template.as_deref() != Some(template) {
            return Err(CodReceiptBlocker::TemplateMismatch);
        }
        if metadata.external_reference_proven != Some(true) {
            return Err(CodReceiptBlocker::ReferenceUnproven);
        }
        // 恢复证明必须关联不可变发送尝试及唯一 MID，不能只凭 proof 标签资金化。
        let exact = (metadata.proof.as_deref() == Some("sent_exact_payload_v1")
            && metadata.sent_verified == Some(true)
            && metadata.settlement_obligation_id.as_deref() == Some(payment.to_string().as_str()))
            || self.frozen_sent_proven;
        if !exact {
            return Err(CodReceiptBlocker::SentProofUnproven);
        }
        Ok(())
    }

    /// 由冻结结算事实重建期望变量；缺列、越界或与买方金额不符都失败关闭。
    fn expected_payload(
        &self,
        template: &'static str,
        payment: Uuid,
        buyer: i64,
        cod: i64,
    ) -> Result<ExpectedPayload, CodReceiptBlocker> {
        let mut variables = CodReceiptVariables {
            reference: payment.to_string(),
            amount: cod.to_string(),
            item_name: None,
            pricing_mode: None,
            hold_ppm: None,
            adjustment_ppm: None,
            winning_bid: None,
            adjusted_price: None,
            pool_credit: None,
        };
        let equipment_secret_refs = if self.carrier_credit == 0 {
            variables.item_name = Some(self.equipment_name.clone());
            self.apply_pricing(&mut variables, buyer)?;
            vec![CodReceiptSecretRef {
                kind: "auction_equipment_key".to_owned(),
                id: self.equipment_id.to_string(),
            }]
        } else {
            Vec::new()
        };
        Ok(ExpectedPayload {
            template,
            variables,
            credits: self.carrier_credit,
            cod,
            equipment_secret_refs,
        })
    }

    /// 新明细变量只从同一事务的冻结结算事实重建；旧三变量载荷保持原证明。
    fn apply_pricing(
        &self,
        variables: &mut CodReceiptVariables,
        buyer: i64,
    ) -> Result<(), CodReceiptBlocker> {
        let legacy = variables.item_name.is_some()
            && variables.pricing_mode.is_none()
            && variables.hold_ppm.is_none()
            && variables.adjustment_ppm.is_none()
            && variables.winning_bid.is_none()
            && variables.adjusted_price.is_none()
            && variables.pool_credit.is_none()
            && self.payload_variables_are_legacy();
        if legacy {
            return Ok(());
        }
        let line =
            self.settlement_line.as_ref().ok_or(CodReceiptBlocker::PricingEndPriceMissing)?;
        let hold_ppm = line.hold_ppm;
        let adjustment_ppm = line.adjustment_ppm;
        if line.buyer_amount.value() != buyer {
            return Err(CodReceiptBlocker::PricingBuyerMismatch);
        }
        if !(0..=SETTLEMENT_PPM).contains(&hold_ppm) {
            return Err(CodReceiptBlocker::PricingHoldInvalid);
        }
        let mode =
            SettlementMode::from_name(self.settlement_mode.as_deref().unwrap_or("hold_curve_v1"))
                .ok_or(CodReceiptBlocker::PricingModeInvalid)?;
        line.validate_for_mode(mode).ok().ok_or(CodReceiptBlocker::PricingLineInvalid)?;
        if mode == SettlementMode::HoldCurveV2 {
            variables.pricing_mode = Some("hold_curve_v2".to_owned());
        }
        variables.hold_ppm = Some(hold_ppm.to_string());
        variables.adjustment_ppm = Some(adjustment_ppm.to_string());
        variables.winning_bid = Some(line.end_price.value().to_string());
        let pool_credit =
            line.buyer_pool_credit().ok().ok_or(CodReceiptBlocker::PricingPoolInvalid)?;
        let adjusted_price = buyer
            .checked_add(pool_credit.value())
            .ok_or(CodReceiptBlocker::PricingAdjustedOverflow)?;
        variables.adjusted_price = Some(adjusted_price.to_string());
        variables.pool_credit = Some(pool_credit.value().to_string());
        Ok(())
    }

    /// 只有原三个变量键才允许省略新明细；完整载荷等值校验仍拒绝缺项、未知项与数值漂移。
    fn payload_variables_are_legacy(&self) -> bool {
        serde_json::from_value::<CodReceiptVariables>(self.payload_variables_value())
            .map(|variables| {
                variables.item_name.is_some()
                    && variables.pricing_mode.is_none()
                    && variables.hold_ppm.is_none()
                    && variables.adjustment_ppm.is_none()
                    && variables.winning_bid.is_none()
                    && variables.adjusted_price.is_none()
                    && variables.pool_credit.is_none()
            })
            .unwrap_or(false)
    }

    fn payload_variables_value(&self) -> serde_json::Value {
        self.payload.get("variables").cloned().unwrap_or(serde_json::Value::Null)
    }
}
