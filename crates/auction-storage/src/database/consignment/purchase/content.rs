//! 首轮邮件内容的注入点：模板名、规范主题、正文摘要与载体装备由装配根按 5C 的冻结模板提供。
//!
//! 存储层不依赖邮件渲染（在 `auction-integrations`），也不自造文本；金额、Credits、COD、
//! 轮引用与期限由本模块按合同图推出并在 `create_first_round_in_tx` 里再核，内容来源只供文本与摘要。

use auction_application::consignment::ConsignmentError;
use auction_domain::consignment::dispatch::FrozenDispatchIdentity;
use auction_domain::consignment::{ConsignmentObligation, ListingPrice};
use auction_domain::{SettlementObligationKind, SettlementWorld};
use auction_time::Timestamp;

/// 冻结首轮内容所需的合同事实；不含装备 key、URL 或任何凭据。
#[derive(Debug, Clone, Copy)]
pub struct FirstDispatchRequest<'a> {
    pub world: SettlementWorld,
    pub price: ListingPrice,
    /// 轮引用，即轮 ID 的小写带连字符 UUID。
    pub round_ref: &'a str,
    /// 付款期限 D0。
    pub payment_deadline_at: Timestamp,
    /// 装备显示名，来自登记时的安全快照。
    pub item_name: &'a str,
    /// 规范装备编号。
    pub canonical_eid: &'a str,
    /// 首轮付款义务（COD 原件或付款载体）。
    pub payment: &'a ConsignmentObligation,
}

/// 首轮的模板、规范主题、正文摘要与标的 / 载体装备 ID。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstDispatchContent {
    pub template: String,
    pub subject: String,
    pub body_digest: [u8; 32],
    /// main 必须恰为 `[canonical_eid]`；isekai 为付款载体装备。
    pub equipment_ids: Vec<String>,
}

/// 冻结撤回件首轮内容所需的合同事实；不含装备 key、URL 或任何凭据。
#[derive(Debug, Clone, Copy)]
pub struct WithdrawalDispatchRequest<'a> {
    /// 原件所在世界。
    pub world: SettlementWorld,
    /// 轮引用，即轮 ID 的小写带连字符 UUID。
    pub round_ref: &'a str,
    /// 装备显示名，来自登记时的安全快照。
    pub item_name: &'a str,
    /// 规范装备编号。
    pub canonical_eid: &'a str,
    /// 撤回合同唯一的返还义务（零金额、零载体）。
    pub obligation: &'a ConsignmentObligation,
}

/// 由装配根实现：渲染冻结模板得出首轮内容。同步、无 I/O、不得失败后留下副作用。
pub trait FirstDispatchContentSource: Send + Sync {
    /// 内容不可得时返回 `Unavailable`：整笔购买回滚，调用方以同命令 ID 重试。
    fn content(
        &self,
        request: &FirstDispatchRequest<'_>,
    ) -> Result<FirstDispatchContent, ConsignmentError>;

    /// 撤回件首轮内容（模板 `consignment_equipment_withdrawal`）；不可得时返回 `Unavailable`，
    /// 整笔撤回（或兑现意愿的取得）回滚，调用方以同命令 ID 重试。
    fn withdrawal_content(
        &self,
        request: &WithdrawalDispatchRequest<'_>,
    ) -> Result<FirstDispatchContent, ConsignmentError>;
}

/// 组装冻结身份：数字与轮引用由合同推出，文本与摘要来自内容来源。
///
/// 保护不变量 6：main 的标的装备必须就是挂单的规范装备，否则失败关闭，不发错件。
pub(super) fn frozen_identity(
    request: &FirstDispatchRequest<'_>,
    content: FirstDispatchContent,
    rights_holder_identity: &str,
    bot_uid: &str,
) -> Result<FrozenDispatchIdentity, ConsignmentError> {
    if request.world == SettlementWorld::Main
        && content.equipment_ids.as_slice() != [request.canonical_eid.to_owned()]
    {
        return Err(ConsignmentError::Unavailable);
    }
    // 冻结身份的世界是派发世界，不是合同世界：isekai 的付款载体在 main 派发（与图写入的履约行
    // `dispatch_world` 同一规则，`contracts::source::dispatch_world_for`；D9 §4.4）。取合同世界会让首轮写入
    // 核对派发世界时失败关闭，isekai 购买整笔回滚（不变量 6）。内容渲染仍按合同世界选模板。
    let world = match request.payment.kind {
        SettlementObligationKind::WinnerPaymentCarrierCod => SettlementWorld::Main,
        _ => request.world,
    };
    let credits = i64::from(request.payment.carrier_credit);
    let cod = request
        .payment
        .amount
        .value()
        .checked_add(credits)
        .ok_or(ConsignmentError::InvalidDepositRequest)?;
    Ok(FrozenDispatchIdentity {
        rights_holder_identity: rights_holder_identity.to_owned(),
        bot_uid: bot_uid.to_owned(),
        world,
        subject: content.subject,
        template: content.template,
        body_digest: content.body_digest,
        round_ref: request.round_ref.to_owned(),
        d0: request.payment_deadline_at,
        credits,
        cod,
        equipment_ids: content.equipment_ids,
    })
}
