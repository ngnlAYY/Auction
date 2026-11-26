//! 寄售派发轮的只读领取观察（PIWQ）：按已绑定的 Sent MID 读那一封邮件的详情，判断收件人是否已领取全部资产
//! （main 原件 COD 的付款与原件、isekai 载体付款、原件交割、卖款或退款的 Credits）。
//!
//! 不变量 4、6、7：
//! - 本入口只读：取只读会话读 Sent 详情，不取写锁、不 POST、不 Take、不改 compose。
//! - 收件人、主题与“回执前的原正文摘要”须逐字等于起写冻结值（无回执块时为整页正文摘要）；仍有剩余资产为“未领取”；
//!   读取失败、会话失效或身份不符一律“无法确定”，调用方照旧等待。
//! - 资产全空时还须有服务端回执块且逐项相符（D9 §2.3）：装备回执条数等于冻结件数（带 EID 的须属冻结集合且不重复），
//!   Credits 回执恰一条且金额等于冻结 Credits（冻结无 Credits 则不得有），`CoD Paid` 等于冻结 COD（冻结无 COD 则不得有），
//!   正文含系统退回附言即不认；回执缺失或格式不符是“无法确定”，不猜测为已领取。
//! - 回执核对排除“空信”（先空后 RTS 等无回执形状），但真实样本未到位，所以本验证器版本仍带 `-unsigned` 后缀，
//!   签收清单为空；在签收（成对脱敏样本）之前，Worker 不会调用本入口，存储也不认它的证据。
//! - 证据摘要只含验证器版本、世界、Sent MID 与冻结请求的安全投影，不含观察时刻：同一领取事实重读得到同一摘要，
//!   证据写口据此判重放。收件人、主题只进入本次比对，日志只记世界与固定分类码。

use std::fmt;

use auction_time::Timestamp;
use sha2::{Digest, Sha256};

use super::{SentRequestParts, sent_request_digest};
use crate::clients::mooglemail::parser::parse_canonical_credits;
use crate::clients::mooglemail::transport::types::MoogleMailDeliveryTransport;
use crate::clients::mooglemail::{
    MoogleMailOperation, MoogleMailSentListEntry, MoogleMailSentObservation, MoogleMailSentReceipt,
    MoogleMailWorld,
};

/// 寄售派发领取观察的验证器版本；未签收（D9 §2.3、R5-C10）。
pub const CONSIGNMENT_DISPATCH_COLLECTED_VALIDATOR_VERSION: &str =
    "consignment_dispatch_collected_v2-unsigned";

/// 一次观察的目标：已绑定的 Sent MID 与起写冻结的 Send 身份；`Debug` 只输出世界与 MID。
pub struct ConsignmentCollectionTarget {
    /// 发信世界。
    pub world: MoogleMailWorld,
    /// 已绑定的 Sent MID。
    pub sent_mid: String,
    /// 起写事务冻结的 `sending_at`（只用于构造详情读取的摘要，不参与判定）。
    pub sending_at: Timestamp,
    /// 冻结收件人。
    pub recipient: String,
    /// 冻结主题。
    pub subject: String,
    /// 冻结正文的规范摘要。
    pub body_digest: [u8; 32],
    /// 冻结 Credits。
    pub credits: i64,
    /// 冻结 COD。
    pub cod: i64,
    /// 冻结附件（按文本升序）。
    pub equipment_ids: Vec<String>,
}

impl fmt::Debug for ConsignmentCollectionTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConsignmentCollectionTarget")
            .field("world", &self.world)
            .field("sent_mid", &self.sent_mid)
            .finish_non_exhaustive()
    }
}

/// 观察结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsignmentCollection {
    /// 身份逐字相符、资产已全部领取。
    Collected {
        /// 产出观察的验证器版本。
        validator_version: &'static str,
        /// 证据摘要（同一领取事实重读不变）。
        evidence_digest: [u8; 32],
    },
    /// 身份相符、仍有剩余资产。
    Pending,
    /// 读取失败、会话失效或身份不符：无法确定。
    Unavailable,
}

impl MoogleMailDeliveryTransport {
    /// 只读观察一封已绑定 Sent 的寄售派发是否已被领取（见模块说明）；绝不写入。
    pub async fn observe_consignment_collection(
        &self,
        target: &ConsignmentCollectionTarget,
    ) -> ConsignmentCollection {
        let world = target.world;
        let session =
            match self.credentials.begin_read(world, MoogleMailOperation::SentDetail).await {
                Ok(session) => session,
                Err(error) => {
                    tracing::warn!(
                        world = world.as_str(),
                        code = error.category.as_str(),
                        "寄售领取观察取读会话失败"
                    );
                    return ConsignmentCollection::Unavailable;
                }
            };
        let entry = MoogleMailSentListEntry {
            external_mail_id: target.sent_mid.clone(),
            recipient: target.recipient.clone(),
            subject: target.subject.clone(),
            sent_at: target.sending_at,
        };
        match self.sender.inspect_sent(session.cookie(), world, &entry, Timestamp::now()).await {
            Ok(observation) => match session.check_current().await {
                Ok(()) => classify(target, &observation),
                Err(error) => {
                    tracing::warn!(
                        world = world.as_str(),
                        code = error.category.as_str(),
                        "寄售领取观察会话已失效"
                    );
                    ConsignmentCollection::Unavailable
                }
            },
            Err(error) => {
                if error.is_session_expired()
                    && let Err(invalidate) = session.invalidate_current().await
                {
                    tracing::warn!(
                        world = world.as_str(),
                        code = invalidate.category.as_str(),
                        "寄售领取观察撤销过期会话失败"
                    );
                }
                tracing::warn!(
                    world = world.as_str(),
                    code = error.category.as_str(),
                    "寄售领取观察读取失败"
                );
                ConsignmentCollection::Unavailable
            }
        }
    }
}

/// 身份逐字相符才判定；仍有剩余为未领取；全空且回执逐项相符才是已领取（不变量 6：不符不猜测）。
fn classify(
    target: &ConsignmentCollectionTarget,
    observation: &MoogleMailSentObservation,
) -> ConsignmentCollection {
    // 领取后正文末尾多出回执块：身份比对用回执前的原正文摘要，没有回执块则用整页摘要。
    let original_digest = match &observation.receipt {
        MoogleMailSentReceipt::Parsed { original_body_digest, .. } => *original_body_digest,
        MoogleMailSentReceipt::None | MoogleMailSentReceipt::Malformed => observation.body_digest,
    };
    if observation.external_mail_id != target.sent_mid
        || observation.recipient != target.recipient
        || observation.subject != target.subject
        || original_digest != target.body_digest
    {
        tracing::warn!(
            world = target.world.as_str(),
            code = "collection_identity_mismatch",
            "寄售领取观察的 Sent 身份与冻结值不符"
        );
        return ConsignmentCollection::Unavailable;
    }
    if !observation.collected {
        return ConsignmentCollection::Pending;
    }
    if observation.returned_notice || !receipt_matches_target(target, &observation.receipt) {
        tracing::warn!(
            world = target.world.as_str(),
            code = "collection_receipt_mismatch",
            "寄售领取观察的服务端回执与冻结值不符"
        );
        return ConsignmentCollection::Unavailable;
    }
    ConsignmentCollection::Collected {
        validator_version: CONSIGNMENT_DISPATCH_COLLECTED_VALIDATOR_VERSION,
        evidence_digest: collected_digest(target),
    }
}

/// 回执逐项核对冻结资产：装备条数与 EID、Credits 恰一条且等额、`CoD Paid` 等于冻结 COD。
fn receipt_matches_target(
    target: &ConsignmentCollectionTarget,
    receipt: &MoogleMailSentReceipt,
) -> bool {
    let MoogleMailSentReceipt::Parsed { removed, cod_paid, .. } = receipt else {
        return false;
    };
    let mut credits = Vec::new();
    let mut equipment = Vec::new();
    for marker in removed {
        match marker.name.strip_suffix(" Credits").and_then(parse_canonical_credits) {
            Some(amount) if marker.canonical_eid.is_none() => credits.push(amount),
            _ => equipment.push(marker),
        }
    }
    let credits_ok =
        if target.credits > 0 { credits == [target.credits] } else { credits.is_empty() };
    let cod_ok = if target.cod > 0 { *cod_paid == Some(target.cod) } else { cod_paid.is_none() };
    let eids_ok = equipment.iter().all(|marker| {
        marker.canonical_eid.as_ref().is_none_or(|eid| target.equipment_ids.contains(eid))
    });
    // 带 EID 的回执不得重复（无 EID 的同名回执在解析阶段已按整行去重）。
    let mut seen: Vec<&String> =
        equipment.iter().filter_map(|marker| marker.canonical_eid.as_ref()).collect();
    seen.sort_unstable();
    let unique = seen.windows(2).all(|pair| pair[0] != pair[1]);
    credits_ok && cod_ok && eids_ok && unique && equipment.len() == target.equipment_ids.len()
}

/// 证据摘要：版本、世界、Sent MID 与冻结请求的安全投影。
fn collected_digest(target: &ConsignmentCollectionTarget) -> [u8; 32] {
    let request = sent_request_digest(&SentRequestParts {
        recipient: &target.recipient,
        subject: &target.subject,
        body_digest: target.body_digest,
        credits: target.credits,
        cod: target.cod,
        equipment_ids: &target.equipment_ids,
    });
    let mut hasher = Sha256::new();
    hasher.update(b"auction.consignment.dispatch.collected.v1\0");
    hasher.update(CONSIGNMENT_DISPATCH_COLLECTED_VALIDATOR_VERSION.as_bytes());
    hasher.update([0]);
    hasher.update(target.world.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(target.sent_mid.as_bytes());
    hasher.update([0]);
    hasher.update(request);
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use auction_domain::consignment::RemovedMarker;
    use auction_time::Timestamp;

    use super::{
        CONSIGNMENT_DISPATCH_COLLECTED_VALIDATOR_VERSION, ConsignmentCollection,
        ConsignmentCollectionTarget, classify,
    };
    use crate::clients::mooglemail::{
        MoogleMailSentObservation, MoogleMailSentReceipt, MoogleMailWorld,
    };

    /// 冻结目标：一封带 COD 与一件装备的假原件派发。
    fn target() -> ConsignmentCollectionTarget {
        ConsignmentCollectionTarget {
            world: MoogleMailWorld::Main,
            sent_mid: "9001".to_owned(),
            sending_at: Timestamp::parse_from_rfc3339("2026-01-01T00:00:00Z").expect("时刻"),
            recipient: "FakeBuyer".to_owned(),
            subject: "Fake subject".to_owned(),
            body_digest: [1; 32],
            credits: 0,
            cod: 100,
            equipment_ids: vec!["42".to_owned()],
        }
    }

    /// 与冻结身份逐字相符的 Sent 详情；`collected` 时剩余资产全零，否则仍是冻结形状。
    fn observation(collected: bool) -> MoogleMailSentObservation {
        MoogleMailSentObservation {
            external_mail_id: "9001".to_owned(),
            recipient: "FakeBuyer".to_owned(),
            subject: "Fake subject".to_owned(),
            remaining_credits: 0,
            cod_pending: if collected { 0 } else { 100 },
            remaining_equipment_ids: if collected { Vec::new() } else { vec!["42".to_owned()] },
            collected,
            body_digest: [1; 32],
            receipt: MoogleMailSentReceipt::None,
            returned_notice: false,
        }
    }

    /// 已领取且带与冻结目标（COD 100、一件装备 42）逐项相符的回执；原正文摘要等于冻结摘要。
    fn receipted() -> MoogleMailSentObservation {
        let mut seen = observation(true);
        seen.body_digest = [9; 32];
        seen.receipt = MoogleMailSentReceipt::Parsed {
            original_body_digest: [1; 32],
            removed: vec![marker("Fake Shoes", Some("42"))],
            cod_paid: Some(100),
        };
        seen
    }

    fn marker(name: &str, eid: Option<&str>) -> RemovedMarker {
        RemovedMarker { name: name.to_owned(), canonical_eid: eid.map(str::to_owned) }
    }

    fn set_receipt(
        seen: &mut MoogleMailSentObservation,
        removed: Vec<RemovedMarker>,
        cod: Option<i64>,
    ) {
        seen.receipt =
            MoogleMailSentReceipt::Parsed { original_body_digest: [1; 32], removed, cod_paid: cod };
    }

    /// 身份任一项不符（Sent MID、收件人、主题、正文摘要）：即使已全部领取也无法确定，不产出证据摘要。
    #[test]
    fn identity_mismatch_is_unavailable_without_digest() {
        let mismatches: [fn(&mut MoogleMailSentObservation); 4] = [
            |seen| seen.external_mail_id = "9002".to_owned(),
            |seen| seen.recipient = "OtherBuyer".to_owned(),
            |seen| seen.subject = "Other subject".to_owned(),
            |seen| seen.body_digest = [2; 32],
        ];
        for mismatch in mismatches {
            let mut seen = observation(true);
            mismatch(&mut seen);
            assert_eq!(classify(&target(), &seen), ConsignmentCollection::Unavailable);
        }
    }

    /// 身份相符、仍有剩余资产：未领取。
    #[test]
    fn remaining_assets_are_pending() {
        assert_eq!(classify(&target(), &observation(false)), ConsignmentCollection::Pending);
    }

    /// 身份相符、全部领取：已领取，带本验证器版本；同一领取重读得到同一摘要。
    #[test]
    fn collected_twice_yields_the_same_digest() {
        let first = classify(&target(), &receipted());
        let ConsignmentCollection::Collected { validator_version, evidence_digest } = first else {
            panic!("全部领取应判为已领取");
        };
        assert_eq!(validator_version, CONSIGNMENT_DISPATCH_COLLECTED_VALIDATOR_VERSION);
        assert_ne!(evidence_digest, [0; 32]);
        assert_eq!(classify(&target(), &receipted()), first);
    }

    /// 全空但没有回执（空信、先空后 RTS 等形状）、回执格式不符或带系统退回附言：无法确定，不产出证据。
    #[test]
    fn empty_without_valid_receipt_is_unavailable() {
        let mut malformed = observation(true);
        malformed.receipt = MoogleMailSentReceipt::Malformed;
        let mut returned = receipted();
        returned.returned_notice = true;
        for seen in [observation(true), malformed, returned] {
            assert_eq!(classify(&target(), &seen), ConsignmentCollection::Unavailable);
        }
    }

    /// 回执与冻结值逐项核对：装备条数或 EID 不符、COD 金额不符或缺失、Credits 多余或不等、原正文摘要不符，都不认。
    #[test]
    fn receipt_items_must_match_the_frozen_target() {
        let cases: [(Vec<RemovedMarker>, Option<i64>); 7] = [
            (vec![], Some(100)),
            (vec![marker("Fake Shoes", Some("43"))], Some(100)),
            (vec![marker("Fake Shoes", Some("42")), marker("Fake Hat", None)], Some(100)),
            (vec![marker("Fake Shoes", Some("42"))], Some(99)),
            (vec![marker("Fake Shoes", Some("42"))], None),
            (vec![marker("Fake Shoes", Some("42")), marker("5 Credits", None)], Some(100)),
            (vec![marker("Fake Shoes", None)], Some(100)),
        ];
        let name_only_case = cases[6].clone();
        for (removed, cod) in cases.into_iter().take(6) {
            let mut seen = receipted();
            set_receipt(&mut seen, removed, cod);
            assert_eq!(classify(&target(), &seen), ConsignmentCollection::Unavailable);
        }
        // 无 EID 的名称回执按条数核对，等于冻结件数即认。
        let (removed, cod) = name_only_case;
        let mut name_only = receipted();
        set_receipt(&mut name_only, removed, cod);
        assert!(matches!(classify(&target(), &name_only), ConsignmentCollection::Collected { .. }));
        let mut digest = receipted();
        digest.receipt = MoogleMailSentReceipt::Parsed {
            original_body_digest: [2; 32],
            removed: vec![marker("Fake Shoes", Some("42"))],
            cod_paid: Some(100),
        };
        assert_eq!(classify(&target(), &digest), ConsignmentCollection::Unavailable);
    }

    /// 冻结带 Credits 时回执须恰一条且等额；冻结无 COD 时不得出现 CoD Paid。
    #[test]
    fn credits_and_cod_receipts_follow_the_frozen_amounts() {
        let mut credits_target = target();
        credits_target.cod = 0;
        credits_target.credits = 49_000;
        let mut seen = receipted();
        set_receipt(
            &mut seen,
            vec![marker("Fake Shoes", Some("42")), marker("49,000 Credits", None)],
            None,
        );
        assert!(matches!(
            classify(&credits_target, &seen),
            ConsignmentCollection::Collected { .. }
        ));
        set_receipt(
            &mut seen,
            vec![marker("Fake Shoes", Some("42")), marker("48,000 Credits", None)],
            None,
        );
        assert_eq!(classify(&credits_target, &seen), ConsignmentCollection::Unavailable);
        set_receipt(&mut seen, vec![marker("Fake Shoes", Some("42"))], None);
        assert_eq!(classify(&credits_target, &seen), ConsignmentCollection::Unavailable);
        set_receipt(
            &mut seen,
            vec![marker("Fake Shoes", Some("42")), marker("49,000 Credits", None)],
            Some(7),
        );
        assert_eq!(classify(&credits_target, &seen), ConsignmentCollection::Unavailable);
    }
}
