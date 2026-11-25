//! 寄售发送邮件的规范主题与系统退件来源认证。
//!
//! 七个寄售主题基名全部以 `Consignment ` 开头，与拍卖的四个基名（`source.rs`）互不相交：
//! 拍卖识别函数对寄售主题返回 `None`，本模块的识别函数对拍卖主题返回 `None`，寄售退件因此不会
//! 被拍卖 owner 当成“本地查不到义务的退款”建成拍卖案件。

use auction_domain::SettlementId;
use uuid::Uuid;

use super::source::SYSTEM_RETURN_SENDER;

/// 系统退信加在原主题前的前缀（ASCII 小写形式），与拍卖退件一致。
const RETURNED_SUBJECT_PREFIX: &str = "[rts] ";

/// 寄售发送邮件的七种主题种类；与七个发送模板一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConsignmentSubjectKind {
    EquipmentCod,
    PaymentCarrier,
    EquipmentHandoff,
    SellerPayout,
    BuyerRefund,
    EquipmentWithdrawal,
    PaymentReminder,
}

impl ConsignmentSubjectKind {
    /// 全部种类，顺序固定。
    pub const ALL: [Self; 7] = [
        Self::EquipmentCod,
        Self::PaymentCarrier,
        Self::EquipmentHandoff,
        Self::SellerPayout,
        Self::BuyerRefund,
        Self::EquipmentWithdrawal,
        Self::PaymentReminder,
    ];

    /// 规范主题基名；已发布后不可修改（系统退件按它认证来源）。
    pub const fn base(self) -> &'static str {
        match self {
            Self::EquipmentCod => "Consignment equipment COD",
            Self::PaymentCarrier => "Consignment payment carrier",
            Self::EquipmentHandoff => "Consignment equipment handoff",
            Self::SellerPayout => "Consignment seller payout",
            Self::BuyerRefund => "Consignment buyer refund",
            Self::EquipmentWithdrawal => "Consignment equipment withdrawal",
            Self::PaymentReminder => "Consignment payment reminder",
        }
    }
}

/// 寄售邮件的规范主题：`"{基名} [{obligation_id}]"`，UUID 为小写带连字符形式。
pub fn canonical_consignment_subject(
    kind: ConsignmentSubjectKind,
    obligation_id: SettlementId,
) -> String {
    format!("{} [{obligation_id}]", kind.base())
}

/// 发件人恰为系统且主题为 `[RTS] ` + 寄售规范主题时，返回主题种类与义务 ID。主题按 ASCII
/// 不区分大小写比较，UUID 必须是规范小写形式；拍卖主题、无前缀主题与玩家来信都返回 `None`。
pub fn system_returned_consignment_obligation_id(
    sender: &str,
    subject: &str,
) -> Option<(ConsignmentSubjectKind, Uuid)> {
    if sender != SYSTEM_RETURN_SENDER {
        return None;
    }
    let subject = subject.to_ascii_lowercase();
    let (base, encoded) = subject.strip_prefix(RETURNED_SUBJECT_PREFIX)?.rsplit_once(" [")?;
    let encoded = encoded.strip_suffix(']')?;
    let kind = ConsignmentSubjectKind::ALL
        .into_iter()
        .find(|kind| kind.base().eq_ignore_ascii_case(base))?;
    let obligation_id = Uuid::parse_str(encoded).ok()?;
    (!obligation_id.is_nil() && obligation_id.to_string() == encoded)
        .then_some((kind, obligation_id))
}

#[cfg(test)]
mod tests {
    use auction_domain::SettlementId;
    use uuid::Uuid;

    use super::{
        ConsignmentSubjectKind, canonical_consignment_subject,
        system_returned_consignment_obligation_id,
    };
    use crate::ports::settlement::returns::source::SYSTEM_RETURN_SENDER;

    const ID: &str = "0f3c8f60-1111-4a2b-8a1d-6f2f5f4b9c11";

    fn obligation() -> SettlementId {
        SettlementId::from_uuid(Uuid::parse_str(ID).unwrap())
    }

    /// 七个主题经系统退回后都能认回原种类与义务 ID（不变量 6：来源认证闭集）。
    #[test]
    fn returned_subjects_round_trip_for_every_kind() {
        for kind in ConsignmentSubjectKind::ALL {
            let subject = canonical_consignment_subject(kind, obligation());
            assert_eq!(subject, format!("{} [{ID}]", kind.base()));
            assert_eq!(
                system_returned_consignment_obligation_id(
                    SYSTEM_RETURN_SENDER,
                    &format!("[RTS] {subject}")
                ),
                Some((kind, Uuid::parse_str(ID).unwrap()))
            );
        }
    }

    /// 玩家发件人、缺 RTS 前缀、大写 UUID、零 UUID 与拍卖主题一律不认。
    #[test]
    fn returned_subject_requires_system_sender_prefix_and_canonical_uuid() {
        let subject =
            canonical_consignment_subject(ConsignmentSubjectKind::BuyerRefund, obligation());
        assert!(
            system_returned_consignment_obligation_id("Player", &format!("[RTS] {subject}"))
                .is_none()
        );
        assert!(
            system_returned_consignment_obligation_id(SYSTEM_RETURN_SENDER, &subject).is_none()
        );
        // 与拍卖识别一致：整条主题先转小写再比较，但 UUID 必须是带连字符的规范形式。
        let simple = format!("[RTS] Consignment buyer refund [{}]", ID.replace('-', ""));
        assert!(system_returned_consignment_obligation_id(SYSTEM_RETURN_SENDER, &simple).is_none());
        let nil = format!("[RTS] Consignment buyer refund [{}]", Uuid::nil());
        assert!(system_returned_consignment_obligation_id(SYSTEM_RETURN_SENDER, &nil).is_none());
        let auction = format!("[RTS] Auction buyer refund [{ID}]");
        assert!(
            system_returned_consignment_obligation_id(SYSTEM_RETURN_SENDER, &auction).is_none()
        );
    }
}
