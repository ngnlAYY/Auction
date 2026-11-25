//! 受限权利通知派发结果的账本回执（`deliver_right_notice`）：与结果行同事务追加。
//!
//! 保护不变量 1、3、7、8：命令 ID 由工作 ID 与通知种类派生，结果行（每工作至多一行）与回执、审计同事务；
//! 回执只带工作 ID 与通知种类，不复现挑战码、定位符或正文；重放只回放、不写第二行。

use auction_application::consignment::{RecoveryOperation, deliver_right_notice_command};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use super::ledger::{Arbitration, LedgerCommand, Step, Target, arbitrate, insert_success};
use crate::database::consignment::commands::errors::WriteError;

/// 结果码（`sent/unknown/suppressed`）对应的回执处置；未知结果码失败关闭。
fn disposition_of(result: &str) -> Result<&'static str, WriteError> {
    match result {
        "sent" => Ok("notice_sent"),
        "unknown" => Ok("notice_unknown"),
        "suppressed" => Ok("notice_suppressed"),
        _ => Err(WriteError::invariant("consignment_notice_result_code")),
    }
}

/// 在写入结果行的同一事务里追加回执与审计。
pub(crate) async fn record_notice_receipt(
    conn: &mut PgConnection,
    work_id: Uuid,
    notice_kind: &str,
    result: &str,
) -> Result<(), WriteError> {
    let disposition = disposition_of(result)?;
    let (command_id, digest) = deliver_right_notice_command(work_id, notice_kind);
    let ledger = LedgerCommand {
        command_id: command_id.as_uuid(),
        operation: RecoveryOperation::DeliverRightNotice,
        target: Target::NoticeWork(work_id),
        digest: &digest,
    };
    match arbitrate(conn, &ledger).await? {
        Step::Done(Arbitration::Replay(_)) => Ok(()),
        Step::Done(Arbitration::First) => {
            insert_success(
                conn,
                &ledger,
                disposition,
                json!({ "work_id": work_id.to_string(), "notice_kind": notice_kind }),
            )
            .await
        }
        Step::Rejected(rejection) => Err(WriteError::from(rejection.into_error())),
    }
}
