// Sent / Read Mail 扫描检查点的持久化端口。
//
// 检查点只保存“可继续前缀扫描的最高 MID 与列表时间”和最近一次完整校验时间，
// 让进程重启后不必重新完整扫描。它不是发送、收取、已读、领取或资金证明；
// Inbox 仍以 `mail_poll_state.cursor_value` 为唯一权威游标，恢复队列独立登记。
// 作用域不匹配、行损坏或读取失败都必须回退完整扫描，不能把旧检查点当作本轮观测。

use std::{future::Future, pin::Pin};

use uuid::Uuid;

use crate::errors::ApplicationError;
use crate::ports::mail::{MailScanCheckpoint, MailScanStream};

pub type MailScanCheckpointFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ApplicationError>> + Send + 'a>>;

/// 检查点的稳定作用域；同一凭据的旧版本、另一世界或另一 endpoint 不得复用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailScanCheckpointKey {
    pub credential_id: Uuid,
    /// 凭据行当前 `state_version`；凭据被停用、改名或重新启用都会使旧检查点失效。
    pub credential_version: i64,
    pub world_id: String,
    /// 持久化只服务 Sent 与 Read Mail；Inbox 由持久游标拥有。
    pub stream: MailScanStream,
    /// 远端 endpoint 的 origin；配置切换到另一站点时不能沿用旧水位。
    pub endpoint_scope: String,
}

impl MailScanCheckpointKey {
    /// 闭集校验；无效作用域只能退回内存或完整扫描。
    pub fn validate(&self) -> bool {
        !self.credential_id.is_nil()
            && self.credential_version > 0
            && matches!(self.world_id.as_str(), "main" | "isekai")
            && matches!(self.stream, MailScanStream::Sent | MailScanStream::ReadMail)
            && auction_domain::valid_text(&self.endpoint_scope, 512)
    }
}

/// 扫描检查点的持久化端口；读写失败只影响增量优化，不改变发送或读取结果。
pub trait MailScanCheckpointStore: Send + Sync {
    /// 只返回作用域完全匹配的检查点；行缺失或损坏由适配器给出 `None`/错误。
    fn load<'a>(
        &'a self,
        key: &'a MailScanCheckpointKey,
    ) -> MailScanCheckpointFuture<'a, Option<MailScanCheckpoint>>;

    /// 覆盖同一 `(credential_id, world, stream)` 的检查点；新作用域写成新行内容。
    fn save<'a>(
        &'a self,
        key: &'a MailScanCheckpointKey,
        checkpoint: &'a MailScanCheckpoint,
    ) -> MailScanCheckpointFuture<'a, ()>;
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::ports::mail::scan_checkpoint as parent;

    fn key(stream: MailScanStream) -> MailScanCheckpointKey {
        MailScanCheckpointKey {
            credential_id: Uuid::from_u128(7),
            credential_version: 3,
            world_id: "main".to_owned(),
            stream,
            endpoint_scope: "https://hentaiverse.org".to_owned(),
        }
    }

    #[test]
    fn key_is_closed_to_inbox_and_invalid_scope() {
        assert!(key(MailScanStream::Sent).validate());
        assert!(key(MailScanStream::ReadMail).validate());
        assert!(!key(MailScanStream::Inbox).validate());

        let mut invalid = key(MailScanStream::Sent);
        invalid.credential_id = Uuid::nil();
        assert!(!invalid.validate());
        invalid = key(MailScanStream::Sent);
        invalid.credential_version = 0;
        assert!(!invalid.validate());
        invalid = key(MailScanStream::Sent);
        invalid.world_id = "other".to_owned();
        assert!(!invalid.validate());
        invalid = key(MailScanStream::Sent);
        invalid.endpoint_scope = " https://hentaiverse.org".to_owned();
        assert!(!invalid.validate());
        invalid = key(MailScanStream::Sent);
        invalid.endpoint_scope = String::new();
        assert!(!invalid.validate());
    }
}
