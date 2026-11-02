//! 冻结结算现金用途；业务owner先锁并验证claim/依赖/库存，本模块只拥有资金SQL。

use super::{
    ledger::*,
    receipt::{store_receipt, validate_receipt},
    *,
};
use auction_application::FundingCollectedReceipt;
use auction_domain::{FundingSplit, SettlementAmount};
use sha2::{Digest, Sha256};

async fn fund_lot(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    command: &FundingMutation,
    lot_id: Uuid,
    receipt: Option<&FundingCollectedReceipt<'_>>,
    equipment_loss: bool,
) -> Result<(), FundingError> {
    let collected = receipt.is_some();
    let cash = receipt.and_then(|receipt| receipt.evidence.main_cash.as_ref());
    let action = if equipment_loss {
        "operator_equipment_loss"
    } else if collected {
        "collected"
    } else {
        "internal_purchase"
    };
    if replay(tx, account, command, action, lot_id).await? {
        return Ok(());
    }
    let row = sqlx::query("SELECT l.buyer_amount,l.compensation_amount,l.seller_amount,l.tax_amount,p.id AS payment_id,s.id AS seller_id,pf.execution_state,pf.proof_kind,pf.proof_digest FROM public.lot_settlements l JOIN public.settlement_obligations p ON p.lot_id=l.lot_id AND p.obligation_kind IN ('winner_equipment_cod','winner_payment_carrier_cod') JOIN public.settlement_obligations s ON s.lot_id=l.lot_id AND s.obligation_kind='seller_payout' JOIN public.settlement_obligation_fulfillments pf ON pf.obligation_id=p.id WHERE l.lot_id=$1")
        .bind(lot_id).fetch_one(&mut **tx).await.map_err(storage)?;
    let split = FundingSplit {
        buyer: SettlementAmount::new(row.try_get("buyer_amount").map_err(storage)?)
            .map_err(invalid)?,
        compensation: SettlementAmount::new(row.try_get("compensation_amount").map_err(storage)?)
            .map_err(invalid)?,
        seller: SettlementAmount::new(row.try_get("seller_amount").map_err(storage)?)
            .map_err(invalid)?,
        tax: SettlementAmount::new(row.try_get("tax_amount").map_err(storage)?).map_err(invalid)?,
    }
    .validate()
    .map_err(invalid)?;
    if equipment_loss {
        auction_domain::equipment_loss_funding(split, account.records_only(), true)
            .map_err(invalid)?;
    }
    let payment_id: Uuid = row.try_get("payment_id").map_err(storage)?;
    let seller_id: Uuid = row.try_get("seller_id").map_err(storage)?;
    if collected
        && (row.try_get::<String, _>("execution_state").map_err(storage)? != "satisfied"
            || row.try_get::<Option<String>, _>("proof_kind").map_err(storage)?.as_deref()
                != Some("mooglemail_sent_collected")
            || row.try_get::<Option<Vec<u8>>, _>("proof_digest").map_err(storage)?.is_none())
    {
        return Err(FundingError::Unproven);
    }
    // lot全局唯一，不因账户/凭据切换重做付款分配。
    sqlx::query("INSERT INTO public.settlement_funding_lots(lot_id,uid,payment_obligation_id,source_kind) VALUES($1,$2,$3,$4)")
        .bind(lot_id).bind(account.uid()).bind(payment_id).bind(if equipment_loss {"operator_equipment_loss"} else if collected {"collected"} else {"internal"})
        .execute(&mut **tx).await.map_err(storage)?;
    let seller = allocation(tx, account.uid(), "seller", Some(lot_id), Some(seller_id)).await?;
    let tax = allocation(tx, account.uid(), "tax", Some(lot_id), None).await?;
    if collected {
        // 当前同session精确实收绑定由调用方证明；不能用旧collected或Balance单独调用。
        let receipt = receipt.ok_or(FundingError::Unproven)?;
        validate_receipt(tx, account, receipt, payment_id, split.buyer.value()).await?;
        let sent = receipt.evidence.sent_collection.as_ref().ok_or(FundingError::Unproven)?;
        let source = source(
            tx,
            account.uid(),
            "collected_payment",
            payment_id,
            split.buyer.value(),
            &sent.digest,
        )
        .await?;
        store_receipt(tx, source, receipt).await?;
        issue(tx, account.uid(), command, source, seller, split.buyer.value()).await?;
    } else if account.records_only() {
        // 原lot冻结B的有限会计来源，不读取个人现金，也不创建可任意消费的proprietary预算。
        let source = source(
            tx,
            account.uid(),
            if equipment_loss {
                "record_operator_equipment_loss"
            } else {
                "record_internal_purchase"
            },
            payment_id,
            split.buyer.value(),
            &command.request_digest,
        )
        .await?;
        issue(tx, account.uid(), command, source, seller, split.buyer.value()).await?;
    } else {
        let proprietary = allocation(tx, account.uid(), "proprietary", None, None).await?;
        transfer(tx, account.uid(), command, proprietary, seller, split.buyer.value()).await?;
    }
    if account.records_only() {
        if split.compensation.value() > 0 {
            // C同样只来自该lot冻结值；唯一source_key和资金命令阻止重放再次登记。
            let source = source(
                tx,
                account.uid(),
                "record_compensation",
                payment_id,
                split.compensation.value(),
                &command.request_digest,
            )
            .await?;
            issue(tx, account.uid(), command, source, seller, split.compensation.value()).await?;
        }
    } else {
        let compensation = allocation(tx, account.uid(), "compensation", None, None).await?;
        transfer(tx, account.uid(), command, compensation, seller, split.compensation.value())
            .await?;
    }
    transfer(tx, account.uid(), command, seller, tax, split.tax.value()).await?;
    if let Some(cash) = cash.filter(|_| !account.records_only()) {
        let local:i64=sqlx::query_scalar("SELECT COALESCE(sum(amount),0)::bigint FROM public.settlement_funding_allocations WHERE uid=$1 AND state<>'spent'")
            .bind(account.uid()).fetch_one(&mut **tx).await.map_err(storage)?;
        if local > cash.credits {
            return Err(FundingError::Unavailable);
        }
    }
    if let Some(receipt) = receipt {
        crate::validate_mail_evidence_time(tx, receipt.evidence).await.map_err(evidence_error)?;
    }
    complete(tx, account, command, action, lot_id).await
}

/// strict_cash从既有自有/补偿用途转移；record_only按该lot冻结B/C登记有限会计来源。
/// 两种模式均不重算B+C=S+T，同一付款只登记一次，不能创建任意P/C消费额度。
pub async fn fund_internal_purchase(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    command: &FundingMutation,
    lot_id: Uuid,
) -> Result<(), FundingError> {
    fund_lot(tx, account, command, lot_id, None, false).await
}

/// 已验证装备损失的人工责任；只支持 record_only，不制造 collected receipt。
pub async fn fund_equipment_loss(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    command: &FundingMutation,
    lot_id: Uuid,
) -> Result<(), FundingError> {
    if !account.records_only() || !account.ready() {
        return Err(FundingError::Unavailable);
    }
    fund_lot(tx, account, command, lot_id, None, true).await
}

/// 调用者必须将当前session实收/UID绑定证明与此动作同事务持久化。
pub async fn fund_collected_payment(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    command: &FundingMutation,
    receipt: &FundingCollectedReceipt<'_>,
) -> Result<(), FundingError> {
    let payment_obligation_id = receipt.payment_obligation_id;
    let sent = receipt.evidence.sent_collection.as_ref().ok_or(FundingError::Unproven)?;
    // 仅稳定来源字段参与重放；新一轮观测时间/generation不改变原命令负载。
    let digest = Sha256::digest(
        serde_json::to_vec(&serde_json::json!({
            "version":1,"business_digest":command.request_digest,"uid":account.uid(),
            "payment":payment_obligation_id,"delivery":receipt.delivery_id,
            "mid":sent.external_mail_id,"buyer":receipt.buyer_credits
        }))
        .map_err(invalid)?,
    )
    .into();
    let command = FundingMutation { command_id: command.command_id, request_digest: digest };
    let lot: Uuid = sqlx::query_scalar("SELECT lot_id FROM public.settlement_obligations WHERE id=$1 AND obligation_kind IN ('winner_equipment_cod','winner_payment_carrier_cod')")
        .bind(payment_obligation_id).fetch_one(&mut **tx).await.map_err(storage)?;
    fund_lot(tx, account, &command, lot, Some(receipt), false).await
}

/// 原seller用途转换，不新增收入、不自动再投资。
pub async fn retain_funded_seller(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    command: &FundingMutation,
    seller_obligation_id: Uuid,
) -> Result<(), FundingError> {
    if replay(tx, account, command, "retain", seller_obligation_id).await? {
        return Ok(());
    }
    let row = sqlx::query("SELECT a.id,a.lot_id,a.amount FROM public.settlement_funding_allocations a JOIN public.settlement_funding_lots l ON l.lot_id=a.lot_id WHERE a.uid=$1 AND a.obligation_id=$2 AND a.purpose='seller' AND a.state='available' AND l.disposition='seller'")
        .bind(account.uid()).bind(seller_obligation_id).fetch_one(&mut **tx).await.map_err(storage)?;
    let lot: Uuid = row.try_get("lot_id").map_err(storage)?;
    let credit = allocation(tx, account.uid(), "retained", Some(lot), None).await?;
    transfer(
        tx,
        account.uid(),
        command,
        row.try_get("id").map_err(storage)?,
        credit,
        row.try_get("amount").map_err(storage)?,
    )
    .await?;
    sqlx::query("UPDATE public.settlement_funding_lots SET disposition='retained' WHERE lot_id=$1")
        .bind(lot)
        .execute(&mut **tx)
        .await
        .map_err(storage)?;
    complete(tx, account, command, "retain", seller_obligation_id).await
}

/// 仅既有EquipmentRecovered退款门激活后转移；seller与refund严格互斥。
pub async fn refund_funded_purchase(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    command: &FundingMutation,
    refund_obligation_id: Uuid,
    internal: bool,
) -> Result<(), FundingError> {
    let action = if internal { "refund_internal" } else { "refund_external" };
    if replay(tx, account, command, action, refund_obligation_id).await? {
        return Ok(());
    }
    let row = sqlx::query("SELECT o.lot_id,o.amount,l.buyer_amount,l.compensation_amount,l.tax_amount,f.source_kind FROM public.settlement_obligations o JOIN public.settlement_recoveries r ON r.buyer_refund_obligation_id=o.id JOIN public.settlement_obligations s ON s.id=r.seller_payout_obligation_id JOIN public.settlement_obligation_fulfillments sf ON sf.obligation_id=s.id JOIN public.lot_settlements l ON l.lot_id=o.lot_id JOIN public.settlement_funding_lots f ON f.lot_id=o.lot_id WHERE o.id=$1 AND o.obligation_kind='buyer_refund' AND o.business_state='ready' AND r.stage='refunding' AND r.resolution='equipment_recovered' AND r.custody_proof_digest IS NOT NULL AND s.business_state='blocked' AND sf.execution_state='pending' AND sf.delivery_id IS NULL AND f.uid=$2 AND f.disposition='seller'")
        .bind(refund_obligation_id).bind(account.uid()).fetch_one(&mut **tx).await.map_err(storage)?;
    let lot: Uuid = row.try_get("lot_id").map_err(storage)?;
    let buyer: i64 = row.try_get("buyer_amount").map_err(storage)?;
    if row.try_get::<i64, _>("amount").map_err(storage)? != buyer {
        return Err(FundingError::Conflict);
    }
    let seller_row = sqlx::query("SELECT id,amount FROM public.settlement_funding_allocations WHERE uid=$1 AND lot_id=$2 AND purpose='seller' AND state='available'")
        .bind(account.uid()).bind(lot).fetch_one(&mut **tx).await.map_err(storage)?;
    let seller: Uuid = seller_row.try_get("id").map_err(storage)?;
    let tax = allocation(tx, account.uid(), "tax", Some(lot), None).await?;
    let tax_amount: i64 = row.try_get("tax_amount").map_err(storage)?;
    // 撤销本lot原S/T分配，将B返原用途、C返受限补偿；不更改经济池。
    transfer(tx, account.uid(), command, tax, seller, tax_amount).await?;
    let target = if internal {
        if !account.records_only()
            && row.try_get::<String, _>("source_kind").map_err(storage)? == "internal"
        {
            allocation(tx, account.uid(), "proprietary", None, None).await?
        } else {
            // 外部来源的bot退款仅留存，不能凭退款自动授权再投资。
            allocation(tx, account.uid(), "retained", Some(lot), None).await?
        }
    } else {
        allocation(tx, account.uid(), "refund", Some(lot), Some(refund_obligation_id)).await?
    };
    transfer(tx, account.uid(), command, seller, target, buyer).await?;
    let compensation = allocation(tx, account.uid(), "compensation", None, None).await?;
    transfer(
        tx,
        account.uid(),
        command,
        seller,
        compensation,
        row.try_get("compensation_amount").map_err(storage)?,
    )
    .await?;
    sqlx::query("UPDATE public.settlement_funding_lots SET disposition='refund' WHERE lot_id=$1")
        .bind(lot)
        .execute(&mut **tx)
        .await
        .map_err(storage)?;
    complete(tx, account, command, action, refund_obligation_id).await
}
