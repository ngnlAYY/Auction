//! 寄售收件就绪心跳的运行时输入读取。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use auction_application::consignment::MailReadinessPolicy;
use auction_application::core::RequestHash;
use auction_config::AppConfig;
use auction_integrations::mooglemail::{MoogleMailGateway, MoogleMailWorld};
use auction_jobs::consignment::MailEvidenceReadinessRuntime;
use auction_storage::DatabaseStore;

/// 为已装配 gateway 的世界读取一次认证会话并生成就绪心跳输入。
///
/// 读取失败只跳过该世界的就绪注册；读取器本身仍已装配，存量处置继续由调用方判定。
pub async fn readiness_runtimes(
    config: &AppConfig,
    store: Arc<DatabaseStore>,
    gateways: &HashMap<MoogleMailWorld, Arc<MoogleMailGateway>>,
) -> Vec<MailEvidenceReadinessRuntime> {
    let mut runtimes = Vec::new();
    let worlds = config.worker.mooglemail.configured_worlds();
    // 单次启动共用一个 30 秒上限，但按候选世界均分：就绪会话只在此处读取一次，
    // 前一个世界的网络超时不得把后面的世界推到已过期的截止时间，否则整个运行期都不注册。
    let per_world =
        std::time::Duration::from_secs(30) / u32::try_from(worlds.len().max(1)).unwrap_or(1);
    for world_config in worlds {
        let Some(world) = MoogleMailWorld::parse(&world_config.world_id) else { continue };
        let Some(gateway) = gateways.get(&world) else { continue };
        let session = match gateway.readiness_session(world, Instant::now() + per_world).await {
            Ok(session) => session,
            Err(error) => {
                tracing::warn!(
                    world_id = world.as_str(),
                    failure_code = error.category.as_str(),
                    "寄售收件就绪会话读取失败，本次不注册该世界"
                );
                continue;
            }
        };
        let allowlist_mode = if world_config.sender_allowlist_disabled {
            "disabled".to_owned()
        } else {
            "enforced".to_owned()
        };
        let allowed_senders = world_config.sender_allowlist.clone();
        let policy = MailReadinessPolicy {
            policy_digest: readiness_policy_digest(&allowlist_mode, &allowed_senders),
            allowlist_mode,
            allowed_senders,
            validator_versions: Vec::new(),
            detail_ready: true,
            // 第 4 期未签收真实 Take/Return 样本：生产能力保持失败关闭。
            // 打开前还须先有 v2 本人与 Admin 读模型：v1 私有投影不认代管态，整封接受后
            // 本人与 Admin 的寄售列表会整页不可用。
            take_ready: false,
            return_ready: false,
        };
        runtimes.push(MailEvidenceReadinessRuntime {
            store: store.clone(),
            session,
            policy,
            valid_for_seconds: 120,
        });
    }
    runtimes
}

fn readiness_policy_digest(allowlist_mode: &str, allowed_senders: &[String]) -> [u8; 32] {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"auction/consignment/mail-readiness-policy/v1\0");
    bytes.extend_from_slice(&(allowlist_mode.len() as u64).to_be_bytes());
    bytes.extend_from_slice(allowlist_mode.as_bytes());
    bytes.extend_from_slice(&(allowed_senders.len() as u64).to_be_bytes());
    for sender in allowed_senders {
        bytes.extend_from_slice(&(sender.len() as u64).to_be_bytes());
        bytes.extend_from_slice(sender.as_bytes());
    }
    RequestHash::from_canonical_bytes(&bytes).as_bytes()
}
