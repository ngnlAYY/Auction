//! 单笔维护的安全 JSON；不输出 opaque 证据、凭据或远端正文。

use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use super::{super::hex_digest, args::LegacyTargetArgs};

pub(super) fn preview<T: Serialize>(
    operation: &str,
    args: &LegacyTargetArgs,
    version: i64,
    digest: &[u8; 32],
    confirmation: String,
    target: &T,
) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": "auction-legacy-settlement/1",
            "operation": operation,
            "mode": "preview",
            "obligation_id": args.obligation_id,
            "expected_uid": args.expected_uid,
            "expected_state_version": version,
            "expected_digest": hex_digest(digest),
            "confirm_write": confirmation,
            "remote_evidence_verified": false,
            "worker_must_be_stopped_for_execute": true,
            "target": target,
        }))?
    );
    Ok(())
}

pub(super) fn execution<T: Serialize>(
    operation: &str,
    obligation_id: Uuid,
    command_id: Uuid,
    outcome: &str,
    result: &T,
) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": "auction-legacy-settlement/1",
            "operation": operation,
            "mode": "execute",
            "obligation_id": obligation_id,
            "command_id": command_id,
            "outcome": outcome,
            "result": result,
        }))?
    );
    Ok(())
}
