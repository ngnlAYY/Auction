//! 人工 Sent 证明的 CLI 边界；只读本地闭集证据并调用应用端口，不访问游戏。

use anyhow::Context;
use auction_application::core::lower_hex;
use auction_application::delivery::{
    DeliverySentConfirmationPort, ManualSentEvidence, SentConfirmationCommand,
    SentConfirmationPreview,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::app::print_json;
use crate::cli::ConfirmSentArgs;
use crate::deployment::ConfigDir;
use crate::support::{files, text};

const MAX_EVIDENCE_BYTES: u64 = 64 * 1024;

pub(super) async fn run(config_dir: &ConfigDir, args: ConfirmSentArgs) -> anyhow::Result<()> {
    let delivery_id = text::command_id(args.delivery_id, "delivery_id")?;
    let bytes = files::read_bounded(&args.evidence, MAX_EVIDENCE_BYTES)
        .context("读取人工 Sent 证据失败")?;
    let evidence: ManualSentEvidence = serde_json::from_slice(&bytes)
        .context("人工 Sent 证据必须符合闭集 ManualSentEvidence JSON")?;
    // 对类型化内容计算摘要；空白与字段顺序不改变确认身份，任何观察值变化均须重新确认。
    let digest = lower_hex(&Sha256::digest(serde_json::to_vec(&evidence)?));
    let command = execution_command(&args, evidence.clone(), &digest)?;
    let (_, store) = super::super::worker_store(
        config_dir,
        auction_storage::DatabaseConnectionPurpose::CliOperations,
    )
    .await?;
    let result = async {
        if let Some(command) = command {
            // 不用最新预览阻断原命令重放；端口负责幂等、CAS 与全部匹配规则。
            let result = store.confirm_sent(command).await.context(
                "人工 Sent 确认未确认完成；结果未知时保留原 command-id、证据与全部参数重放核对",
            )?;
            print_json(&json!({
                "outcome": if result.replayed { "replayed" } else { "committed" },
                "result": result,
            }))
        } else {
            let preview = store
                .preview_sent_confirmation(delivery_id, evidence)
                .await
                .context("读取人工 Sent 确认预览失败")?;
            print_preview(&args, &preview, &digest)
        }
    }
    .await;
    store.postgres_pool().close().await;
    result
}

fn execution_command(
    args: &ConfirmSentArgs,
    evidence: ManualSentEvidence,
    digest: &str,
) -> anyhow::Result<Option<SentConfirmationCommand>> {
    if !args.execute {
        return Ok(None);
    }
    let version = args.expected_fencing_version.context("--expected-fencing-version 必填")?;
    if version < 0 {
        anyhow::bail!("--expected-fencing-version 不得为负数");
    }
    let command = SentConfirmationCommand {
        command_id: text::command_id(
            args.command_id.context("--command-id 必填")?,
            "--command-id",
        )?,
        delivery_id: args.delivery_id,
        expected_fencing_version: version,
        actor: text::actor(args.actor.as_deref().context("--actor 必填")?)?.to_owned(),
        operator_identity: Some(super::super::operator_identity::capture()?),
        reason: args.reason.clone().context("--reason 必填")?,
        evidence,
        accept_collected_assets: args.accept_collected_assets,
        accept_attempt_history: args.accept_attempt_history,
    };
    let expected = confirmation(
        command.delivery_id,
        version,
        &format!(
            "mooglemail:{}:send:{}",
            command.evidence.world_id, command.evidence.external_mail_id
        ),
        digest,
        command.accept_collected_assets,
        command.accept_attempt_history,
    );
    text::confirmation(args.confirm.as_deref().context("--confirm 必填")?, &expected)?;
    Ok(Some(command))
}

fn print_preview(
    args: &ConfirmSentArgs,
    preview: &SentConfirmationPreview,
    digest: &str,
) -> anyhow::Result<()> {
    let confirm = confirmation(
        preview.delivery_id,
        preview.expected_fencing_version,
        &preview.external_reference,
        digest,
        preview.requires_collected_asset_acceptance,
        preview.requires_attempt_history_acceptance,
    );
    let mut argv = vec![
        "auction-cli".to_owned(),
        "--config-dir".to_owned(),
        "<CONFIG_DIR>".to_owned(),
        "settlement".to_owned(),
        "confirm-sent".to_owned(),
        preview.delivery_id.to_string(),
        "--evidence".to_owned(),
        args.evidence.to_string_lossy().into_owned(),
        "--execute".to_owned(),
        "--command-id".to_owned(),
        "<STABLE_COMMAND_UUID>".to_owned(),
        "--expected-fencing-version".to_owned(),
        preview.expected_fencing_version.to_string(),
        "--actor".to_owned(),
        "<ACTOR>".to_owned(),
        "--reason".to_owned(),
        "<EVIDENCE_SOURCE_AND_OPERATOR_RESPONSIBILITY>".to_owned(),
    ];
    if preview.requires_collected_asset_acceptance {
        argv.push("--accept-collected-assets".to_owned());
    }
    if preview.requires_attempt_history_acceptance {
        argv.push("--accept-attempt-history".to_owned());
    }
    argv.extend(["--confirm".to_owned(), confirm.clone()]);
    print_json(&json!({
        "outcome": "preview",
        "preview": preview,
        "evidence_sha256": digest,
        "execution": { "confirm": confirm, "argv_template": argv },
        "note": "只读数据库预览；执行事务重新校验资格与版本。人工证明使用 sent_operator_confirmed_v1，不冒充自动精确资产证明；已收取装备或货款及旧尝试关联分别需要显式接受责任；corrects_admin_failure=true 表示以 Sent 证据更正原 Admin 失败结论，原失败命令与审计保留。装备确认后的 COD 实收和卖方付款仍走正常管线；货款确认仅按原资金 owner 结清既有 unknown 支出，不新建付款或投递。领取仍须正常核验。argv_template 是独立参数数组，禁止 eval。",
    }))
}

fn confirmation(
    delivery_id: Uuid,
    version: i64,
    external_reference: &str,
    digest: &str,
    accept_collected_assets: bool,
    accept_attempt_history: bool,
) -> String {
    format!(
        "CONFIRM SENT {delivery_id} {version} {external_reference} {digest} COLLECTED {accept_collected_assets} ATTEMPT HISTORY {accept_attempt_history}"
    )
}
