//! 只输出安全账户投影及闭集执行结果，不打印原始凭据、远端正文或 opaque 证据。

use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

const SCHEMA: &str = "auction-bot-funds-record-only/1";

/// 输出数据库投影与精确确认模板，明确尚未核验远端身份。
pub(super) fn preview(
    uid: &str,
    version: i64,
    digest: &[u8; 32],
    confirmation: String,
    preview: &impl Serialize,
) -> anyhow::Result<()> {
    let digest: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema":SCHEMA,"mode":"preview","target_policy":"record_only",
            "expected_uid":uid,"expected_version":version,"preview_digest":digest,
            "confirm_write":confirmation,"remote_evidence_verified":false,"observes_balance":false,
            "worker_must_be_stopped_for_execute":true,"preview":preview
        }))?
    );
    Ok(())
}

/// 仅输出安全结果或闭集错误，调用方另以非零退出码标明未完成。
pub(super) fn execution(
    uid: &str,
    command_id: Uuid,
    outcome: &str,
    result: &impl Serialize,
) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema":SCHEMA,"mode":"execute","target_policy":"record_only",
            "expected_uid":uid,"command_id":command_id,"outcome":outcome,"result":result
        }))?
    );
    Ok(())
}
