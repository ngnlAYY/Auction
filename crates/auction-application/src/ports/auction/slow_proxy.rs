//! 慢速代理到期扫描与幂等执行端口。
//! Worker 只负责发现持久化 due generation 并触发命令；竞价锁、资格检查、领域计划、命令账本
//! 和审计仍由应用服务与存储事务共同拥有。

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use auction_domain::{CommandId, CompetitionPlan, LotId, ProxyAuthorizationId};

use super::{ApplicationFuture, CompetitionTransactionError, RequestHash};
use crate::ApplicationError;

/// 旧版命令账本用于慢速代理执行结果的稳定命令种类。
pub const SLOW_PROXY_COMMAND_KIND: &str = "execute_slow_response";

/// Worker 从 lot 持久化调度字段读取的一次慢速代理到期 generation。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlowProxyDue {
    pub lot_id: LotId,
    pub authorization_id: ProxyAuthorizationId,
    pub generation: u64,
    pub due_at: DateTime<Utc>,
}

impl SlowProxyDue {
    /// 校验扫描结果，并按 Auction-BF 的稳定算法生成命令身份和请求摘要。
    pub fn into_command(self) -> Result<ExecuteSlowProxyCommand, CompetitionTransactionError> {
        self.validate()?;
        let generation = i64::try_from(self.generation).map_err(|_| {
            CompetitionTransactionError::Failed("慢速代理世代超出旧命令范围".to_owned())
        })?;
        let due_nanos = self.due_at.timestamp_nanos_opt().ok_or_else(|| {
            CompetitionTransactionError::Failed("慢速代理到期时间超出命令范围".to_owned())
        })?;
        let kind = format!("slow_proxy:{generation}");
        let command_id = legacy_worker_command_id(
            "auction-worker",
            &kind,
            self.authorization_id.as_uuid(),
            due_nanos,
        );
        let request_hash = legacy_request_hash(self, generation)?;
        Ok(ExecuteSlowProxyCommand {
            command_id,
            lot_id: self.lot_id,
            authorization_id: self.authorization_id,
            generation: self.generation,
            due_at: self.due_at,
            request_hash,
        })
    }

    pub fn validate(self) -> Result<(), CompetitionTransactionError> {
        if self.lot_id.is_nil()
            || self.authorization_id.is_nil()
            || self.generation == 0
            || self.due_at.timestamp_nanos_opt().is_none()
        {
            return Err(CompetitionTransactionError::Failed("慢速代理 Worker 候选无效".to_owned()));
        }
        Ok(())
    }
}

/// Worker 提交给竞价 owner 的慢速代理执行命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecuteSlowProxyCommand {
    pub command_id: CommandId,
    pub lot_id: LotId,
    pub authorization_id: ProxyAuthorizationId,
    pub generation: u64,
    pub due_at: DateTime<Utc>,
    pub request_hash: RequestHash,
}

impl ExecuteSlowProxyCommand {
    /// 校验可信 Worker 入口仍未绕过稳定命令身份与 due generation 边界。
    pub fn validate(self) -> Result<(), CompetitionTransactionError> {
        if self.command_id.is_nil()
            || self.lot_id.is_nil()
            || self.authorization_id.is_nil()
            || self.generation == 0
            || self.generation > i64::MAX as u64
            || self.due_at.timestamp_nanos_opt().is_none()
            || self.request_hash.as_bytes() == [0; 32]
        {
            return Err(CompetitionTransactionError::Failed("慢速代理执行命令无效".to_owned()));
        }
        Ok(())
    }
}

/// 无锁读取到期慢速代理候选的窄端口。
/// 到期判断必须来自数据库时钟；扫描不取得业务行锁，执行端会在 Serializable/串行写事务中重读
/// lot、代理、用户资格和同一 generation。
pub trait SlowProxyDueReader: Send + Sync {
    fn list_due_slow_proxies(&self, limit: u16) -> ApplicationFuture<'_, Vec<SlowProxyDue>>;

    /// 返回严格晚于 `now` 的最早慢速代理到期时刻，只作 Worker 唤醒提示；执行仍重读全部 fence。
    fn next_slow_proxy_due_at(
        &self,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, Option<DateTime<Utc>>>;
}

/// 慢速代理命令执行端口。实现必须把账本、竞价事实、代理冻结、审计和事件原子提交。
pub trait SlowProxyCommandPort: Send + Sync {
    fn execute_slow_proxy(
        &self,
        command: ExecuteSlowProxyCommand,
    ) -> ApplicationFuture<'_, CompetitionPlan>;
}

/// Worker 用于把旧版明确的 stale candidate 分类为成功跳过，而不吞掉存储或账本错误。
pub fn is_stale_slow_proxy_error(error: &ApplicationError) -> bool {
    matches!(
        error,
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::StaleSlowProxy)
    )
}

fn legacy_worker_command_id(
    namespace: &str,
    kind: &str,
    entity_id: Uuid,
    due_nanos: i64,
) -> CommandId {
    let mut hash = Sha256::new();
    for value in [namespace.as_bytes(), kind.as_bytes(), entity_id.as_bytes()] {
        hash.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
        hash.update(value);
    }
    hash.update(due_nanos.to_be_bytes());
    let digest = hash.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    CommandId::from_uuid(Uuid::from_bytes(bytes))
}

fn legacy_request_hash(
    due: SlowProxyDue,
    generation: i64,
) -> Result<RequestHash, CompetitionTransactionError> {
    #[derive(Serialize)]
    struct LegacySlowProxyRequest {
        #[serde(rename = "LotID")]
        lot_id: [u8; 16],
        #[serde(rename = "AuthorizationID")]
        authorization_id: [u8; 16],
        #[serde(rename = "Generation")]
        generation: i64,
        #[serde(rename = "DueAt")]
        due_at: String,
    }

    let payload = serde_json::to_vec(&LegacySlowProxyRequest {
        lot_id: *due.lot_id.as_uuid().as_bytes(),
        authorization_id: *due.authorization_id.as_uuid().as_bytes(),
        generation,
        due_at: due.due_at.to_rfc3339_opts(SecondsFormat::AutoSi, true),
    })
    .map_err(|_| CompetitionTransactionError::Failed("慢速代理旧版请求摘要编码失败".to_owned()))?;
    let digest = Sha256::digest(payload);
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(&digest);
    Ok(RequestHash::new(bytes))
}
