//! 寄售义务在最终 Send 授权点的受益人判定：先锁受益用户，再由调用方锁履约行后调用判定。
//!
//! 保护不变量 2、4、6：`authorize_entitlement_recipient_in_tx` 要求调用方在同一事务里先以 `FOR SHARE`
//! 锁受益用户行、再锁履约行，然后判定（与结算既定顺序 `users → obligations → … → fulfillments` 一致，
//! 不成环）。本模块只做第一步并告诉调用方义务是否属于寄售合同；拍卖义务不取任何新锁、不改原判定。

use sqlx::PgConnection;
use uuid::Uuid;

use crate::database::{
    EntitlementDecision, EntitlementRequest, authorize_entitlement_recipient_in_tx,
};

/// 义务的来源：决定是否走寄售受益人判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::database) enum SendBeneficiary {
    /// 拍卖义务或义务不存在：维持调用方原有的收件人判定。
    Auction,
    /// 寄售合同义务：受益用户（若能确定）已在本事务以 `FOR SHARE` 锁住。
    Consignment,
}

/// 只读确定义务的寄售受益用户并以 `FOR SHARE` 锁住；必须在锁履约行之前调用。
///
/// 角色与 `authorize_entitlement_recipient_in_tx` 的义务种类映射一致：买方种类取 sale 冻结买方，
/// 卖方种类取合同冻结卖方。合同与 sale 行只追加、运行账号不可锁，所以无锁读取。受益人无法确定时
/// 不锁任何用户，判定会给出保留（失败关闭）。
pub(in crate::database) async fn lock_send_beneficiary(
    connection: &mut PgConnection,
    obligation_id: Uuid,
) -> Result<SendBeneficiary, sqlx::Error> {
    let row: Option<(bool, Option<Uuid>)> = sqlx::query_as(
        "SELECT obligation.contract_id IS NOT NULL,
                CASE
                    WHEN obligation.obligation_kind IN ('winner_equipment_cod',
                         'winner_payment_carrier_cod', 'winner_equipment_handoff', 'buyer_refund')
                        THEN sale.buyer_user_id
                    WHEN obligation.obligation_kind IN ('seller_payout', 'withdrawn_return',
                         'unsold_return')
                        THEN contract.seller_user_id
                END
         FROM public.settlement_obligations AS obligation
         LEFT JOIN public.consignment_contracts AS contract
           ON contract.contract_id = obligation.contract_id
         LEFT JOIN public.consignment_sales AS sale ON sale.contract_id = obligation.contract_id
         WHERE obligation.id = $1",
    )
    .bind(obligation_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some((true, beneficiary)) = row else {
        return Ok(SendBeneficiary::Auction);
    };
    if let Some(user_id) = beneficiary {
        sqlx::query("SELECT 1 FROM public.users WHERE id = $1 FOR SHARE")
            .bind(user_id)
            .execute(&mut *connection)
            .await?;
    }
    Ok(SendBeneficiary::Consignment)
}

/// 在受益用户与履约行都已锁住后判定寄售义务的冻结收件人；只有 `Authorized` 才可继续发送。
///
/// `frozen_recipient` 是本次发送实际使用的收件名（投递行冻结值）。黑名单不是条件；改名、权利
/// 非 active、受益人不一致一律保留权利、零发送（失败关闭）。
pub(in crate::database) async fn authorize_consignment_send(
    connection: &mut PgConnection,
    obligation_id: Uuid,
    frozen_recipient: &str,
) -> Result<EntitlementDecision, sqlx::Error> {
    authorize_entitlement_recipient_in_tx(
        connection,
        &EntitlementRequest {
            obligation_id,
            beneficiary_user_id: None,
            frozen_recipient: frozen_recipient.to_owned(),
        },
    )
    .await
}
