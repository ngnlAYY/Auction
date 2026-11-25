//! 付款提醒用例：规范摘要、系统命令 UUIDv5 与端口命令组装。

use std::sync::Arc;

use auction_domain::CommandId;
use uuid::Uuid;

use super::resolve::{Stream, system_id};
use crate::ports::consignment::{
    ConsignmentError, PaymentReminderStore, RecoveryOperation, RemindContract, ReminderReceipt,
};
use crate::ports::core::RequestHash;

/// 提醒种类；与合同一起构成业务键（D9 §6.2：`remind` = 合同 + 提醒种类）。
const PAYMENT_REMINDER_KIND: &str = "payment_reminder";

fn remind_command(contract_id: Uuid) -> (CommandId, RequestHash) {
    let mut stream = Stream::new(RecoveryOperation::Remind, contract_id);
    stream.text(PAYMENT_REMINDER_KIND);
    let mut key = contract_id.as_bytes().to_vec();
    key.extend_from_slice(&(PAYMENT_REMINDER_KIND.len() as u64).to_be_bytes());
    key.extend_from_slice(PAYMENT_REMINDER_KIND.as_bytes());
    (system_id(RecoveryOperation::Remind, &key), stream.request_hash())
}

/// 付款提醒写用例；只依赖窄端口。
pub struct PaymentReminderService {
    store: Arc<dyn PaymentReminderStore>,
}

impl PaymentReminderService {
    /// 注入付款提醒写端口。
    pub fn new(store: Arc<dyn PaymentReminderStore>) -> Self {
        Self { store }
    }

    /// 对一个合同建立（或作废）付款提醒；每合同一次。
    pub async fn remind(&self, contract_id: Uuid) -> Result<ReminderReceipt, ConsignmentError> {
        let (command_id, request_hash) = remind_command(contract_id);
        self.store.remind(RemindContract { command_id, request_hash, contract_id }).await
    }

    /// 已到期、已证寄出且尚无提醒决定的合同。
    pub async fn due(&self, limit: u32) -> Result<Vec<Uuid>, ConsignmentError> {
        self.store.due_reminders(limit).await
    }
}
