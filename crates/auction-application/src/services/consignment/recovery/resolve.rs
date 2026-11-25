//! 退回处置用例：规范摘要、系统命令 UUIDv5 与端口命令组装。
//!
//! 保护不变量 1、10：同一消费只有一个命令 ID，决策时冻结的自动重列配置进入摘要——同一消费换配置
//! 重试是异请求（`command_id_conflict`），不会另造第二次处置；没有配置摘要（全零）时失败关闭。

use std::sync::Arc;

use auction_domain::CommandId;
use uuid::Uuid;

use crate::ports::consignment::{
    AutoRelistSetting, ConsignmentError, DepositActor, PendingResolution, RecoveryOperation,
    ResolveReceipt, ResolveReturn, ReturnDispositionStore, ScanCursor, ScanPage,
};
use crate::ports::core::RequestHash;
use crate::services::consignment::deposit::encoding::COMMAND_PREFIX_V2;

/// 规范字节构建器：`prefix || text(operation) || text(actor 类型) || text(服务引用) || target || inputs`。
pub(super) struct Stream {
    bytes: Vec<u8>,
}

impl Stream {
    /// 写入前缀、操作、worker actor 与目标 UUID。
    pub(super) fn new(operation: RecoveryOperation, target: Uuid) -> Self {
        let mut stream = Self { bytes: COMMAND_PREFIX_V2.to_vec() };
        stream.text(operation.as_str());
        stream.text(operation.actor_kind());
        stream.text(DepositActor::DEPOSIT_SERVICE_REF);
        stream.uuid(target);
        stream
    }

    pub(super) fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    pub(super) fn uuid(&mut self, value: Uuid) {
        self.bytes.extend_from_slice(value.as_bytes());
    }

    pub(super) fn text(&mut self, value: &str) {
        self.u64(value.len() as u64);
        self.bytes.extend_from_slice(value.as_bytes());
    }

    pub(super) fn digest(&mut self, value: &[u8; 32]) {
        self.bytes.extend_from_slice(value);
    }

    pub(super) fn request_hash(&self) -> RequestHash {
        RequestHash::from_canonical_bytes(&self.bytes)
    }
}

/// 系统命令身份：`UUIDv5(NAMESPACE_URL, prefix || text(operation) || 业务键)`。
pub(super) fn system_id(operation: RecoveryOperation, business_key: &[u8]) -> CommandId {
    let mut input = COMMAND_PREFIX_V2.to_vec();
    input.extend_from_slice(&(operation.as_str().len() as u64).to_be_bytes());
    input.extend_from_slice(operation.as_str().as_bytes());
    input.extend_from_slice(business_key);
    CommandId::from_uuid(Uuid::new_v5(&Uuid::NAMESPACE_URL, &input))
}

/// `resolve_return`：业务键为消费 ID（每个消费只处置一次）；摘要含接管与冻结的自动重列配置。
fn resolve_command(
    consumption_id: Uuid,
    intake_id: Uuid,
    setting: AutoRelistSetting,
) -> (CommandId, RequestHash) {
    let mut stream = Stream::new(RecoveryOperation::ResolveReturn, intake_id);
    stream.uuid(consumption_id);
    stream.u64(u64::from(setting.enabled));
    stream.digest(&setting.config_digest);
    (system_id(RecoveryOperation::ResolveReturn, consumption_id.as_bytes()), stream.request_hash())
}

/// 退回处置写用例；只依赖窄端口。
pub struct ReturnDispositionService {
    store: Arc<dyn ReturnDispositionStore>,
}

impl ReturnDispositionService {
    /// 注入退回处置写端口。
    pub fn new(store: Arc<dyn ReturnDispositionStore>) -> Self {
        Self { store }
    }

    /// 对一次精确 Take 消费作出处置；`setting` 是 Worker 启动时加载的自动重列配置。
    pub async fn resolve(
        &self,
        pending: PendingResolution,
        setting: AutoRelistSetting,
    ) -> Result<ResolveReceipt, ConsignmentError> {
        // 缺配置失败关闭（J12）：没有摘要就无法冻结，也就不能作出带自动重列资格的处置。
        if setting.config_digest == [0; 32] {
            return Err(ConsignmentError::Unavailable);
        }
        let (command_id, request_hash) =
            resolve_command(pending.consumption_id, pending.intake_id, setting);
        self.store
            .resolve_return(ResolveReturn {
                command_id,
                request_hash,
                consumption_id: pending.consumption_id,
                intake_id: pending.intake_id,
                auto_relist: setting,
            })
            .await
    }

    /// 游标之后尚无处置的消费（一页）。
    pub async fn pending(
        &self,
        after: Option<ScanCursor>,
        limit: u32,
    ) -> Result<ScanPage<PendingResolution>, ConsignmentError> {
        self.store.pending_resolutions(after, limit).await
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::resolve_command;
    use crate::ports::consignment::AutoRelistSetting;

    const CONSUMPTION: Uuid = Uuid::from_u128(0x1111);
    const INTAKE: Uuid = Uuid::from_u128(0x2222);

    fn setting(enabled: bool) -> AutoRelistSetting {
        AutoRelistSetting { enabled, config_digest: [7; 32] }
    }

    #[test]
    fn command_id_follows_the_consumption_and_the_digest_follows_the_frozen_setting() {
        let (first_id, first_hash) = resolve_command(CONSUMPTION, INTAKE, setting(false));
        let (again_id, again_hash) = resolve_command(CONSUMPTION, INTAKE, setting(false));
        assert_eq!((first_id, first_hash), (again_id, again_hash));
        let (flipped_id, flipped_hash) = resolve_command(CONSUMPTION, INTAKE, setting(true));
        assert_eq!(flipped_id, first_id, "同一消费只有一个命令 ID");
        assert_ne!(flipped_hash, first_hash, "冻结配置不同是异请求");
    }
}
