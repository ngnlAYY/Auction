//! 结算退件的来源认证：只有系统发件人 `MoogleMail` 以 `[RTS] ` + 原 canonical 结算主题退回的
//! 邮件是结算退件。义务 ID 可由公开的 auction/lot ID 推出，不能单独作为来源证明。

use auction_domain::{SettlementId, SettlementObligationKind};
use uuid::Uuid;

use crate::ports::settlement::fulfillment::canonical_settlement_fulfillment_subject;

/// MoogleMail 系统退信的发件人；精确比较、不忽略大小写，玩家不能使用这个名字发信。
pub const SYSTEM_RETURN_SENDER: &str = "MoogleMail";

/// 系统退信加在原主题前的前缀（ASCII 小写形式）。
const RETURNED_SUBJECT_PREFIX: &str = "[rts] ";

/// 结算邮件 canonical 主题的基名（ASCII 小写），与 `canonical_settlement_fulfillment_subject`
/// 的四种主题一一对应。
const SETTLEMENT_SUBJECT_BASES: [&str; 4] = [
    "auction equipment delivery",
    "auction payment request",
    "auction seller payout",
    "auction buyer refund",
];

/// 发件人恰为系统且主题为 `[RTS] ` + canonical 结算主题时，返回主题里的义务 ID。主题按 ASCII
/// 不区分大小写比较，UUID 必须是规范小写形式；无前缀主题、玩家自拟的同名主题与其他系统退信
/// 都返回 `None`。寄售主题（`Consignment ...`，见 `consignment_source`）的基名不在四个拍卖基名内，
/// 同样返回 `None`，由寄售识别函数另行认证。
pub fn system_returned_settlement_obligation_id(sender: &str, subject: &str) -> Option<Uuid> {
    if sender != SYSTEM_RETURN_SENDER {
        return None;
    }
    let subject = subject.to_ascii_lowercase();
    let (base, encoded) = subject.strip_prefix(RETURNED_SUBJECT_PREFIX)?.rsplit_once(" [")?;
    let encoded = encoded.strip_suffix(']')?;
    if !SETTLEMENT_SUBJECT_BASES.contains(&base) {
        return None;
    }
    let obligation_id = Uuid::parse_str(encoded).ok()?;
    (!obligation_id.is_nil() && obligation_id.to_string() == encoded).then_some(obligation_id)
}

/// 系统退件的主题是否恰为该义务种类的 canonical 主题加 `[RTS] ` 前缀（ASCII 不区分大小写）；
/// 比 [`system_returned_settlement_obligation_id`] 更严格，同时绑定义务种类。
pub fn system_return_matches_obligation(
    sender: &str,
    subject: &str,
    kind: SettlementObligationKind,
    obligation_id: Uuid,
) -> bool {
    sender == SYSTEM_RETURN_SENDER
        && canonical_settlement_fulfillment_subject(kind, SettlementId::from_uuid(obligation_id))
            .is_some_and(|canonical| subject.eq_ignore_ascii_case(&format!("[RTS] {canonical}")))
}

#[cfg(test)]
mod tests {
    use auction_domain::SettlementId;
    use uuid::Uuid;

    use super::{
        SETTLEMENT_SUBJECT_BASES, SYSTEM_RETURN_SENDER, system_returned_settlement_obligation_id,
    };
    use crate::ports::settlement::returns::consignment_source::{
        ConsignmentSubjectKind, canonical_consignment_subject,
    };

    /// 寄售主题基名与拍卖四个基名不相交，拍卖识别函数对任何寄售主题返回 `None`：
    /// 寄售退件不会被拍卖 owner 当作“本地不存在的退款”（不变量 6：来源失败关闭）。
    #[test]
    fn consignment_subjects_are_disjoint_from_auction_and_unrecognized() {
        let obligation = SettlementId::from_uuid(Uuid::from_u128(7));
        for kind in ConsignmentSubjectKind::ALL {
            let base = kind.base().to_ascii_lowercase();
            assert!(base.starts_with("consignment "), "{base}");
            assert!(!SETTLEMENT_SUBJECT_BASES.contains(&base.as_str()), "{base}");
            let subject = canonical_consignment_subject(kind, obligation);
            assert!(
                system_returned_settlement_obligation_id(
                    SYSTEM_RETURN_SENDER,
                    &format!("[RTS] {subject}")
                )
                .is_none(),
                "{subject}"
            );
        }
    }
}
