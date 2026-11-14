//! 草稿清理：仅在完整候选唯一匹配后，先持久提交一次性围栏再授权 POST 丢弃。

use std::time::Instant;

use serde_json::json;

use super::MailboxMaintenanceReport;
use super::MailboxMaintenanceService;
use super::diagnostics::{check_deadline, record_unpersisted_error};
use crate::errors::ApplicationError;
use crate::ports::mail::{
    MoogleMailboxComposeState, MoogleMailboxDiscard, MoogleMailboxSweepCandidate,
    MoogleMailboxUnprovenSend,
};
use crate::ports::operations::ExternalWriteActivity;

impl MailboxMaintenanceService {
    /// 读取当前草稿并在唯一精确匹配后丢弃；任何提前返回都保留本轮已获 Sent 证据。
    pub(super) async fn clean_compose(
        &self,
        world_id: &str,
        candidates: &[MoogleMailboxSweepCandidate],
        deadline: Instant,
        activity: &ExternalWriteActivity,
        report: &mut MailboxMaintenanceReport,
    ) -> Result<(), ApplicationError> {
        let compose = match self.maintenance.compose(world_id).await {
            Ok(compose) => compose,
            Err(error) => {
                record_unpersisted_error("read_compose", world_id, &error);
                report.compose_error = Some(error.to_string());
                return Ok(());
            }
        };
        report.compose_clean = compose.clean;
        if compose.clean {
            return Ok(());
        }
        let mut matches =
            candidates.iter().filter_map(|candidate| candidate.send.as_ref()).filter(|candidate| {
                candidate.world_id == world_id && compose_matches(candidate, &compose)
            });
        let Some(matched) = matches.next() else { return Ok(()) };
        report.compose_matches = 1 + matches.count();
        if report.compose_matches != 1 {
            return Ok(());
        }
        let delivery_id = matched.delivery_id;
        report.matched_delivery = Some(delivery_id);
        check_deadline(deadline)?;
        // 围栏调用开始即受保护；关闭先赢时不消费一次性尝试，保留原持久状态。
        // 已登记的数据库往返与后续外写共用排空截止，登记本身不授权 discard。
        if !activity.enter_critical() {
            return Ok(());
        }
        if !self
            .store
            .begin_compose_discard(
                delivery_id,
                matched.fencing_version,
                matched.send_expectation.as_ref(),
            )
            .await?
        {
            report.compose_error = Some("草稿丢弃已尝试或投递状态已变化，保留人工核对".to_owned());
            return Ok(());
        }
        // 围栏在排队和 POST 前持久提交；其后任意超时或取消都不能自动重新授权 discard。
        check_deadline(deadline)?;
        let outcome = match self.maintenance.discard_compose(world_id, &compose).await {
            Ok(outcome) => outcome,
            Err(error) => {
                record_unpersisted_error("discard_compose", world_id, &error);
                report.compose_error = Some(error.to_string());
                return Ok(());
            }
        };
        report.discarded = outcome == MoogleMailboxDiscard::Discarded;
        if report.discarded {
            self.store
                .record_compose_discard(
                    delivery_id,
                    matched.fencing_version,
                    json!({
                        "observed_at": self.clock.now(),
                        "outcome": "discarded",
                        "matched_reference": matched.reference,
                        "recipient_matches": true,
                        "cod_matches": true,
                    }),
                )
                .await?;
            activity.complete();
        }
        report.compose_clean = outcome == MoogleMailboxDiscard::AlreadyClean || report.discarded;
        Ok(())
    }
}

fn compose_matches(
    candidate: &MoogleMailboxUnprovenSend,
    compose: &MoogleMailboxComposeState,
) -> bool {
    if candidate.operation != "send" {
        return false;
    }
    if let Some(expectation) = &candidate.send_expectation {
        let (Some(expected), Some(observed)) =
            (&expectation.credential, &compose.observation_credential)
        else {
            return false;
        };
        return expectation.validate()
            && expectation.world_id == candidate.world_id
            && expected.credential_id == observed.credential_id
            && expected.credential_version == observed.credential_version
            && expectation.recipient.eq_ignore_ascii_case(&compose.recipient)
            && expectation.subject == compose.subject
            && expectation.body_digest == compose.body_digest
            && compose.cod == Some(expectation.cod)
            && compose.credits == expectation.credits
            && compose.equipment_ids == expectation.equipment_ids;
    }
    // 通知类没有业务 reference；空串会匹配任意草稿，必须显式排除。
    // 旧行也必须核对原装备身份；总附件数还包含 Credits，不能代替 EID 证明。
    !candidate.reference.is_empty()
        && candidate.recipient.eq_ignore_ascii_case(&compose.recipient)
        && compose.cod == Some(candidate.cod)
        && compose.credits == candidate.credits
        && candidate.attachment_ids.windows(2).all(|ids| ids[0] < ids[1])
        && compose.equipment_ids == candidate.attachment_ids
        && (compose.subject.contains(&candidate.reference)
            || compose.body.contains(&candidate.reference))
}
