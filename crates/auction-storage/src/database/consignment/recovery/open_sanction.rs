//! 达阈值处罚的 `open_sanction` 账本回执：与返回事实、处罚、构成项、新购阻断和账号工作同事务追加。
//!
//! 保护不变量 1、3、8：命令 ID 由 `(买家, 周期)` 派生（每个周期至多一项处罚），回执与审计在获证事务内
//! 只追加；同 ID 已有回执只回放、不写第二行，同 ID 异请求按账本冲突失败关闭（D9 §6.2 `open_sanction`）。

use auction_application::consignment::{RecoveryOperation, open_sanction_command};
use serde_json::json;
use sqlx::PgConnection;

use super::ledger::{Arbitration, LedgerCommand, Step, Target, arbitrate, insert_success};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::refusal::OpenedSanction;

/// 在获证事务内写 `sanction_pending` 回执与审计；调用方已写好处罚本身。
pub(crate) async fn record_open_sanction(
    conn: &mut PgConnection,
    opened: &OpenedSanction,
) -> Result<(), WriteError> {
    let (command_id, digest) = open_sanction_command(
        opened.buyer_user_id,
        opened.record.cycle_id,
        opened.record.sanction_id,
    );
    let ledger = LedgerCommand {
        command_id: command_id.as_uuid(),
        operation: RecoveryOperation::OpenSanction,
        target: Target::BuyerCycle(opened.record.cycle_id),
        digest: &digest,
    };
    match arbitrate(conn, &ledger).await? {
        Step::Done(Arbitration::Replay(_)) => Ok(()),
        Step::Done(Arbitration::First) => {
            insert_success(
                conn,
                &ledger,
                "sanction_pending",
                json!({
                    "cycle_id": opened.record.cycle_id.to_string(),
                    "sanction_id": opened.record.sanction_id.to_string(),
                }),
            )
            .await
        }
        Step::Rejected(rejection) => Err(WriteError::from(rejection.into_error())),
    }
}
