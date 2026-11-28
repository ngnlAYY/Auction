//! 寄售派发的只读 Sent 复核：按原 attempt 冻结的 Send 身份，在 `sending_at` 之后的 Sent 列表里找那一封
//! 邮件，供订单复核经既有证据写口补记 Sent 绑定。
//!
//! 不变量 4、6、7：本入口只读——取只读会话、扫描 Sent 摘要并读详情，不取写锁、不 POST、不改 compose；
//! 只有恰好一封详情的收件人、主题与正文摘要逐字等于冻结值、资产（Credits、COD 与附件）为冻结形状或已全部
//! 领取时才算找到；
//! 列表未覆盖到 `sending_at`、候选过多或多封相符一律“无法确定”，不补记。收件人、主题只进入本次比对，
//! 日志只记世界与固定分类码。

use std::fmt;

use auction_application::mail::mail_recipient_is_list_projection;
use auction_time::Timestamp;

use super::{
    CONSIGNMENT_DISPATCH_SENT_VALIDATOR_VERSION, ConsignmentDispatchSent, SentRequestParts,
    evidence_digest, sent_request_digest,
};
use crate::clients::mooglemail::parser::subject_is_list_projection;
use crate::clients::mooglemail::transport::types::MoogleMailDeliveryTransport;
use crate::clients::mooglemail::{
    MoogleMailCookie, MoogleMailError, MoogleMailOperation, MoogleMailSentObservation,
    MoogleMailSentReceipt, MoogleMailWorld,
};

/// 单次复核最多读取的候选详情数；超出时不猜测，按无法确定处理。
const MAXIMUM_CANDIDATES: usize = 16;

/// 一次复核的核对目标：原 attempt 冻结的 Send 身份；`Debug` 只输出世界与时刻。
pub struct ConsignmentDispatchRecheckTarget {
    /// 发信世界。
    pub world: MoogleMailWorld,
    /// 起写事务冻结的 `sending_at`；只在它所在分钟及之后查找。
    pub sending_at: Timestamp,
    /// 冻结收件人。
    pub recipient: String,
    /// 冻结主题。
    pub subject: String,
    /// 冻结正文的规范摘要。
    pub body_digest: [u8; 32],
    /// 冻结 Credits（现金轮的载体 1、卖款 S、退款 P；原件轮为 0）。
    pub credits: i64,
    /// 冻结 COD。
    pub cod: i64,
    /// 冻结附件（按文本升序）。
    pub equipment_ids: Vec<String>,
}

impl fmt::Debug for ConsignmentDispatchRecheckTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConsignmentDispatchRecheckTarget")
            .field("world", &self.world)
            .field("sending_at", &self.sending_at)
            .finish_non_exhaustive()
    }
}

/// 复核结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsignmentDispatchRecheck {
    /// 恰好一封 Sent 邮件与冻结身份逐字相符。
    Found(ConsignmentDispatchSent),
    /// 列表已覆盖到 `sending_at`，没有相符的邮件。
    NotFound,
    /// 读取失败、列表未覆盖、候选过多或多封相符：无法确定。
    Unavailable,
}

impl MoogleMailDeliveryTransport {
    /// 只读复核一封寄售派发是否已在 Sent 里（见模块说明）；绝不写入。
    pub async fn recheck_consignment_dispatch(
        &self,
        target: &ConsignmentDispatchRecheckTarget,
    ) -> ConsignmentDispatchRecheck {
        let world = target.world;
        let session = match self.credentials.begin_read(world, MoogleMailOperation::Sent).await {
            Ok(session) => session,
            Err(error) => {
                tracing::warn!(
                    world = world.as_str(),
                    code = error.category.as_str(),
                    "寄售派发复核取读会话失败"
                );
                return ConsignmentDispatchRecheck::Unavailable;
            }
        };
        match self.find_dispatch_sent(session.cookie(), target).await {
            Ok(outcome) => match session.check_current().await {
                Ok(()) => outcome,
                Err(error) => {
                    tracing::warn!(
                        world = world.as_str(),
                        code = error.category.as_str(),
                        "寄售派发复核会话已失效"
                    );
                    ConsignmentDispatchRecheck::Unavailable
                }
            },
            Err(error) => {
                if error.is_session_expired()
                    && let Err(invalidate) = session.invalidate_current().await
                {
                    tracing::warn!(
                        world = world.as_str(),
                        code = invalidate.category.as_str(),
                        "寄售派发复核撤销过期会话失败"
                    );
                }
                tracing::warn!(
                    world = world.as_str(),
                    code = error.category.as_str(),
                    "寄售派发复核读取失败"
                );
                ConsignmentDispatchRecheck::Unavailable
            }
        }
    }

    async fn find_dispatch_sent(
        &self,
        cookie: &MoogleMailCookie,
        target: &ConsignmentDispatchRecheckTarget,
    ) -> Result<ConsignmentDispatchRecheck, MoogleMailError> {
        let world = target.world;
        let scanned = self.sender.scan_sent_summaries(cookie, world).await?;
        let floor = minute_floor(target.sending_at);
        // 列表只有分钟精度：从 `sending_at` 所在分钟起比较，保留整个重叠分钟。
        // 与应用层 `covers` 同口径：前缀只有覆盖到 sending_at 所在分钟才能证明缺失。
        let covered = scanned.complete && scanned.covered_from.is_none_or(|from| floor >= from);
        let candidates: Vec<_> = scanned
            .summaries
            .iter()
            .filter(|entry| {
                entry.sent_at >= floor
                    && subject_is_list_projection(&entry.subject, &target.subject)
                    && mail_recipient_is_list_projection(&entry.recipient, &target.recipient)
            })
            .collect();
        if candidates.len() > MAXIMUM_CANDIDATES {
            return Ok(ConsignmentDispatchRecheck::Unavailable);
        }
        let mut found = Vec::new();
        for entry in candidates {
            let observation =
                self.sender.inspect_sent(cookie, world, entry, Timestamp::now()).await?;
            if matches_target(&observation, target) {
                found.push(observation.external_mail_id);
            }
        }
        Ok(conclude(target, &found, covered))
    }
}

/// 由相符的 Sent MID 与列表覆盖情况得出结论：只有列表已覆盖到 `sending_at` 所在分钟、且恰好一封相符时
/// 才算找到；覆盖到且零封为找不到；未覆盖（即使已见到一封）或多封相符一律无法确定，不补记（不变量 4、6）。
fn conclude(
    target: &ConsignmentDispatchRecheckTarget,
    found: &[String],
    covered: bool,
) -> ConsignmentDispatchRecheck {
    if !covered {
        return ConsignmentDispatchRecheck::Unavailable;
    }
    match found {
        [sent_mid] => {
            let request = sent_request_digest(&SentRequestParts {
                recipient: &target.recipient,
                subject: &target.subject,
                body_digest: target.body_digest,
                credits: target.credits,
                cod: target.cod,
                equipment_ids: &target.equipment_ids,
            });
            ConsignmentDispatchRecheck::Found(ConsignmentDispatchSent {
                sent_mid: sent_mid.clone(),
                validator_version: CONSIGNMENT_DISPATCH_SENT_VALIDATOR_VERSION,
                evidence_digest: evidence_digest(target.world, sent_mid, &request),
            })
        }
        [] => ConsignmentDispatchRecheck::NotFound,
        _ => ConsignmentDispatchRecheck::Unavailable,
    }
}

/// 详情逐字相符：收件人、主题、正文摘要；资产（剩余 Credits、待付 COD 与附件）仍是冻结形状，或已全部领取
/// （收件人领取后为全零）。
fn matches_target(
    observation: &MoogleMailSentObservation,
    target: &ConsignmentDispatchRecheckTarget,
) -> bool {
    let intact = observation.remaining_credits == target.credits
        && observation.cod_pending == target.cod
        && observation.remaining_equipment_ids == target.equipment_ids;
    // 冻结正文本身可能以“Removed attachment: …”这类模板行结尾，未领取时它也会被解析成回执块，
    // 因此整页摘要与回执前摘要都可能等于冻结摘要：未领取只认整页摘要，已领取两者其一即可。
    let digest_matches = observation.body_digest == target.body_digest
        || (observation.collected
            && matches!(
                &observation.receipt,
                MoogleMailSentReceipt::Parsed { original_body_digest, .. }
                    if *original_body_digest == target.body_digest
            ));
    observation.recipient == target.recipient
        && observation.subject == target.subject
        && digest_matches
        && (intact || observation.collected)
}

fn minute_floor(at: Timestamp) -> Timestamp {
    at - auction_time::Duration::seconds(i64::from(at.to_utc_datetime().second()))
        - auction_time::Duration::nanoseconds(i64::from(at.timestamp_subsec_nanos()))
}

#[cfg(test)]
mod tests {
    use auction_time::Timestamp;

    use super::{ConsignmentDispatchRecheck, ConsignmentDispatchRecheckTarget, conclude};
    use crate::clients::mooglemail::MoogleMailWorld;

    fn target() -> ConsignmentDispatchRecheckTarget {
        ConsignmentDispatchRecheckTarget {
            world: MoogleMailWorld::Main,
            sending_at: Timestamp::parse_from_rfc3339("2026-01-01T00:00:00Z").expect("时刻"),
            recipient: "FakeBuyer".to_owned(),
            subject: "Fake subject".to_owned(),
            body_digest: [1; 32],
            credits: 0,
            cod: 100,
            equipment_ids: vec!["42".to_owned()],
        }
    }

    /// 已覆盖时恰好一封才找到；多封相符无法确定，不能挑一封补记。
    #[test]
    fn only_a_single_match_on_a_covered_list_is_found() {
        let one = ["9001".to_owned()];
        assert!(matches!(conclude(&target(), &one, true), ConsignmentDispatchRecheck::Found(_)));
        let two = ["9001".to_owned(), "9002".to_owned()];
        assert_eq!(conclude(&target(), &two, true), ConsignmentDispatchRecheck::Unavailable);
    }

    /// 列表未覆盖到 `sending_at` 时，即使已见到一封相符也无法确定。
    #[test]
    fn an_uncovered_list_is_never_found() {
        let one = ["9001".to_owned()];
        assert_eq!(conclude(&target(), &one, false), ConsignmentDispatchRecheck::Unavailable);
        assert_eq!(conclude(&target(), &[], false), ConsignmentDispatchRecheck::Unavailable);
    }
}
