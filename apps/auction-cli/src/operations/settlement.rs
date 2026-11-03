//! 装备损失的受审计人工收口；只装配结算端口，不读取 Sent、发信或重建业务资格。

use anyhow::Context;
use auction_application::{
    EquipmentLossCommand, EquipmentLossPreview, SettlementEquipmentLossPort,
};
use serde_json::json;
use uuid::Uuid;

use crate::app::print_json;
use crate::cli::{EquipmentLossArgs, SettlementCommand};
use crate::deployment::ConfigDir;
use crate::support::text;

mod confirm_sent;

pub(crate) async fn run(config_dir: &ConfigDir, command: SettlementCommand) -> anyhow::Result<()> {
    let args = match command {
        SettlementCommand::EquipmentLoss(args) => args,
        SettlementCommand::ConfirmSent(args) => return confirm_sent::run(config_dir, args).await,
    };
    let lot_id = text::command_id(args.lot_id, "lot_id")?;
    // 错误确认或不完整执行声明在连接前拒绝；写入时不得先用新预览阻断原命令重放。
    let command = execution_command(args)?;
    let (_, store) =
        super::worker_store(config_dir, auction_storage::DatabaseConnectionPurpose::CliOperations)
            .await?;
    let result = async {
        if let Some(command) = command {
            let result = store.resolve_equipment_loss(command).await.context(
                "装备损失人工收口未确认完成；结果未知时保留原 command-id 与全部参数重放核对",
            )?;
            print_json(&json!({
                "outcome": if result.replayed { "replayed" } else { "committed" },
                "result": result,
            }))
        } else {
            let preview = store
                .preview_equipment_loss(lot_id)
                .await
                .context("读取装备损失人工收口预览失败")?;
            print_preview(&preview)
        }
    }
    .await;
    store.postgres_pool().close().await;
    result
}

fn execution_command(args: EquipmentLossArgs) -> anyhow::Result<Option<EquipmentLossCommand>> {
    if !args.execute {
        return Ok(None);
    }
    if !args.buyer_compensation_external {
        anyhow::bail!("执行必须明确声明 --buyer-compensation-external，承担买方线下补偿责任");
    }
    let command = EquipmentLossCommand {
        command_id: text::command_id(
            args.command_id.context("--command-id 必填")?,
            "--command-id",
        )?,
        lot_id: args.lot_id,
        actor: text::actor(args.actor.as_deref().context("--actor 必填")?)?.to_owned(),
        reason: args.reason.context("--reason 必填")?,
        funding_uid: args.funding_uid.context("--funding-uid 必填")?,
        expected_recovery_version: args
            .expected_recovery_version
            .context("--expected-recovery-version 必填")?,
        expected_payment_version: args
            .expected_payment_version
            .context("--expected-payment-version 必填")?,
        expected_delivery_fencing_version: args
            .expected_delivery_fencing_version
            .context("--expected-delivery-fencing-version 必填")?,
        expected_seller_amount: args.seller_amount.context("--seller-amount 必填")?,
        buyer_compensation_external: true,
    };
    let expected = confirmation(
        command.lot_id,
        &command.funding_uid,
        command.expected_recovery_version,
        command.expected_payment_version,
        command.expected_delivery_fencing_version,
        command.expected_seller_amount,
    );
    text::confirmation(args.confirm.as_deref().context("--confirm 必填")?, &expected)?;
    Ok(Some(command))
}

fn print_preview(preview: &EquipmentLossPreview) -> anyhow::Result<()> {
    let execution = preview.funding_uid.as_deref().map(|uid| {
        let confirm = confirmation(
            preview.lot_id,
            uid,
            preview.expected_recovery_version,
            preview.expected_payment_version,
            preview.expected_delivery_fencing_version,
            preview.seller_amount,
        );
        // 参数数组无需 shell 转义；操作者替换占位值后按独立参数执行，不能直接 eval。
        let argv = json!([
            "auction-cli",
            "--config-dir",
            "<CONFIG_DIR>",
            "settlement",
            "equipment-loss",
            preview.lot_id.to_string(),
            "--execute",
            "--command-id",
            "<STABLE_COMMAND_UUID>",
            "--actor",
            "<ACTOR>",
            "--reason",
            "<LOSS_REASON_AND_OPERATOR_RESPONSIBILITY>",
            "--funding-uid",
            uid,
            "--expected-recovery-version",
            preview.expected_recovery_version.to_string(),
            "--expected-payment-version",
            preview.expected_payment_version.to_string(),
            "--expected-delivery-fencing-version",
            preview.expected_delivery_fencing_version.to_string(),
            "--seller-amount",
            preview.seller_amount.to_string(),
            "--buyer-compensation-external",
            "--confirm",
            confirm,
        ]);
        json!({ "confirm": confirm, "argv_template": argv })
    });
    print_json(&json!({
        "outcome": "preview",
        "preview": preview,
        "execution": execution,
        "note": "只读数据库预览；资格与版本以执行事务为准。操作者声明装备损失并承担买方线下补偿，不证明买方已获补偿、COD 已收款或卖家已收款。只支持 ready/record_only 资金账户；卖家仍按原冻结金额经正常履约支付，原装备投递永久退休，不得重试。",
    }))
}

fn confirmation(
    lot_id: Uuid,
    funding_uid: &str,
    recovery_version: u64,
    payment_version: u64,
    delivery_fencing_version: i64,
    seller_amount: i64,
) -> String {
    format!(
        "RESOLVE EQUIPMENT LOSS {lot_id} {funding_uid} {recovery_version} {payment_version} \
         {delivery_fencing_version} {seller_amount} EXTERNAL BUYER COMPENSATION"
    )
}
