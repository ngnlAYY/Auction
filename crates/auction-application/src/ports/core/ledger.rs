use std::future::Future;

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use auction_domain::CommandId;

/// 幂等账本使用的固定 SHA-256 请求摘要。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestHash([u8; 32]);

impl RequestHash {
    /// 从已经计算好的 SHA-256 摘要建立值对象。
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// 返回摘要字节，适配器负责写入固定 32 字节字段。
    pub const fn as_bytes(self) -> [u8; 32] {
        self.0
    }

    /// 对已经按具体命令契约冻结的字节序列计算 SHA-256 摘要。
    /// 旧系统已发布的幂等摘要可能不使用通用 envelope；这类兼容编码仍由各命令 owner 构造，
    /// 但最终摘要统一由该值对象建立，避免散落固定长度复制逻辑。
    pub fn from_canonical_bytes(payload: &[u8]) -> Self {
        let digest = Sha256::digest(payload);
        let mut bytes = [0_u8; 32];
        bytes.copy_from_slice(&digest);
        Self::new(bytes)
    }

    /// 按 Go `encoding/json.Marshal` 的默认 HTML 安全规则编码并计算旧版 SHA-256 摘要。
    /// 旧版命令和证明摘要绑定原始 JSON 字节；serde 默认不会转义 `<`、`>`、`&`、U+2028
    /// 与 U+2029，因此兼容 owner 必须统一经过此入口。
    pub fn from_legacy_go_json<T: Serialize>(value: &T) -> Result<Self, serde_json::Error> {
        let encoded = serde_json::to_vec(value)?;
        let mut canonical = Vec::with_capacity(encoded.len());
        let mut index = 0;
        while index < encoded.len() {
            let replacement = match encoded[index] {
                b'<' => Some((br"\u003c".as_slice(), 1)),
                b'>' => Some((br"\u003e".as_slice(), 1)),
                b'&' => Some((br"\u0026".as_slice(), 1)),
                0xe2 if encoded.get(index..index + 3) == Some([0xe2, 0x80, 0xa8].as_slice()) => {
                    Some((br"\u2028".as_slice(), 3))
                }
                0xe2 if encoded.get(index..index + 3) == Some([0xe2, 0x80, 0xa9].as_slice()) => {
                    Some((br"\u2029".as_slice(), 3))
                }
                _ => None,
            };
            if let Some((replacement, consumed)) = replacement {
                canonical.extend_from_slice(replacement);
                index += consumed;
            } else {
                canonical.push(encoded[index]);
                index += 1;
            }
        }
        Ok(Self::from_canonical_bytes(&canonical))
    }

    /// 按稳定的命令种类、聚合标识和请求 DTO 计算 SHA-256 摘要。
    /// 摘要算法属于应用幂等边界而不是 HTTP 层；这样重试、队列和其他入口不会各自定义
    /// 一套容易漂移的 hash 规则。
    pub fn from_request<T: Serialize>(
        command_kind: &str,
        aggregate_id: Uuid,
        request: &T,
    ) -> Result<Self, serde_json::Error> {
        #[derive(Serialize)]
        struct Envelope<'a, T> {
            command_kind: &'a str,
            aggregate_id: Uuid,
            request: &'a T,
        }

        let payload = serde_json::to_vec(&Envelope { command_kind, aggregate_id, request })?;
        Ok(Self::from_canonical_bytes(&payload))
    }
}

/// 命令账本中用于重放的最小记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCommand {
    pub command_id: CommandId,
    pub command_kind: String,
    pub aggregate_id: Uuid,
    pub request_hash: RequestHash,
    pub result_code: String,
    pub result_payload: Vec<u8>,
}

impl StoredCommand {
    /// 校验账本记录对应的非空字段和固定摘要长度。
    pub fn validate(&self) -> Result<(), CommandLedgerError> {
        if self.command_id.is_nil()
            || self.aggregate_id.is_nil()
            || self.command_kind.trim().is_empty()
            || self.result_code.trim().is_empty()
            || self.result_payload.is_empty()
        {
            return Err(CommandLedgerError::InvalidRecord);
        }
        if serde_json::from_slice::<Value>(&self.result_payload)
            .ok()
            .is_none_or(|payload| !payload.is_object())
        {
            return Err(CommandLedgerError::InvalidRecord);
        }
        Ok(())
    }

    pub fn matches_request(
        &self,
        command_kind: &str,
        aggregate_id: Uuid,
        request_hash: RequestHash,
    ) -> bool {
        self.command_kind == command_kind
            && self.aggregate_id == aggregate_id
            && self.request_hash == request_hash
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CommandLedgerError {
    #[error("命令账本记录无效")]
    InvalidRecord,
    #[error("命令 ID 与已有请求负载冲突")]
    Conflict,
    #[error("命令账本存储失败")]
    Storage,
}

/// 命令账本端口。实现必须在持有业务写锁的同一事务中完成查重与写入。
pub trait CommandLedger: Send + Sync {
    fn find(
        &self,
        command_id: CommandId,
    ) -> impl Future<Output = Result<Option<StoredCommand>, CommandLedgerError>> + Send;

    fn insert(
        &self,
        command: StoredCommand,
    ) -> impl Future<Output = Result<(), CommandLedgerError>> + Send;
}
