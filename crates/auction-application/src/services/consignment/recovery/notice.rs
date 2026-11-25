//! 受限权利通知派发结果的系统命令：业务键为通知工作 ID 与通知种类（D9 §6.2 `deliver_right_notice`）。
//!
//! 保护不变量 1：同一工作只有一个命令 ID 与摘要，重放落到同一条回执；派发结果是回执内容，不入摘要。

use auction_domain::CommandId;
use uuid::Uuid;

use super::resolve::{Stream, system_id};
use crate::ports::consignment::RecoveryOperation;
use crate::ports::core::RequestHash;

/// `deliver_right_notice`：账本目标是通知工作，摘要含通知种类。
pub fn deliver_right_notice_command(work_id: Uuid, notice_kind: &str) -> (CommandId, RequestHash) {
    let mut stream = Stream::new(RecoveryOperation::DeliverRightNotice, work_id);
    stream.text(notice_kind);
    let mut key = work_id.as_bytes().to_vec();
    key.extend_from_slice(&(notice_kind.len() as u64).to_be_bytes());
    key.extend_from_slice(notice_kind.as_bytes());
    (system_id(RecoveryOperation::DeliverRightNotice, &key), stream.request_hash())
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::deliver_right_notice_command;

    #[test]
    fn notice_command_follows_work_and_kind_and_is_uuid_v5() {
        let work = Uuid::from_u128(0x7d);
        let (id, hash) = deliver_right_notice_command(work, "challenge");
        assert_eq!(id.as_uuid().get_version_num(), 5);
        assert_eq!((id, hash), deliver_right_notice_command(work, "challenge"));
        assert_ne!(id, deliver_right_notice_command(work, "locator_guide").0);
        assert_ne!(id, deliver_right_notice_command(Uuid::from_u128(0x7e), "challenge").0);
    }
}
