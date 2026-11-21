//! 全局命令账本、首写能力与安全回放。

use auction_application::consignment::{
    ConsignmentError, ListingOperation, RegistrationRequestMac,
};
use auction_application::core::RequestHash;
use serde_json::Value;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::facts::PendingEvent;
use crate::database::consignment::commands::session::CommandTransaction;

/// 仅由受信会话或本地装配提供的演员身份。Worker 应使用固定服务身份。
#[allow(dead_code, reason = "各演员由后续任务 2D / 2F 接线")]
#[derive(Clone, Copy)]
pub(crate) enum Actor<'a> {
    PublicUser(Uuid),
    Admin(&'a str),
    Worker,
    System,
}

impl Actor<'_> {
    pub(super) const fn kind(self) -> &'static str {
        match self {
            Self::PublicUser(_) => "public_user",
            Self::Admin(_) => "admin",
            Self::Worker => "worker",
            Self::System => "system",
        }
    }

    pub(super) fn reference(self) -> String {
        match self {
            Self::PublicUser(seller_user_id) => seller_user_id.to_string(),
            Self::Admin(actor_ref) => actor_ref.to_owned(),
            Self::Worker | Self::System => "consignment".into(),
        }
    }

    pub(super) const fn audit_source(self) -> &'static str {
        match self {
            Self::PublicUser(_) => "public",
            _ => self.kind(),
        }
    }
}

/// 登记的固定目标 new 或已存在/待核的挂单 UUID。
#[allow(dead_code, reason = "由后续任务 2D / 2F 调用")]
#[derive(Clone, Copy)]
pub(crate) enum Target {
    New,
    Listing(Uuid),
}

impl Target {
    fn as_text(self) -> String {
        match self {
            Self::New => "new".into(),
            Self::Listing(listing_id) => listing_id.to_string(),
        }
    }
}

/// 无秘密输入的 SHA-256，或由调用方按逻辑钥算好的登记 MAC。
#[allow(dead_code, reason = "由后续任务 2D / 2F 调用")]
#[derive(Clone, Copy)]
pub(crate) enum RequestProof {
    Digest(RequestHash),
    Registration { replay_key_id: Uuid, mac: RegistrationRequestMac },
}

impl RequestProof {
    fn parts(self) -> (Option<Uuid>, [u8; 32]) {
        match self {
            Self::Digest(hash) => (None, hash.as_bytes()),
            Self::Registration { replay_key_id, mac } => (Some(replay_key_id), *mac.as_bytes()),
        }
    }
}

/// 原始业务状态码、代码及经迁移白名单验证的回执。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct StoredReceipt {
    /// 第一次定案的 HTTP 状态码。
    pub(crate) status: i16,
    /// 第一次定案的稳定业务码。
    pub(crate) code: String,
    /// 第一次定案的 JSONB 回执。
    pub(crate) body: Value,
}

/// 命令比较尚未完成，回执字段在 MAC 或摘要通过前保持私有。
pub(crate) struct PendingReplay {
    replay_key_id: Option<Uuid>,
    digest: RegistrationRequestMac,
    receipt: StoredReceipt,
}

impl PendingReplay {
    /// 取登记账本绑定的逻辑钥。只在演员、操作和目标已经核对后返回。
    #[allow(dead_code, reason = "由后续任务 2D 的登记重放调用")]
    pub(crate) fn replay_key_id(&self) -> Option<Uuid> {
        self.replay_key_id
    }

    /// 不变量 1、7：摘要和登记 MAC 均用常量时间比较。失败不返回回执。
    #[allow(dead_code, reason = "由后续任务 2D / 2F 调用")]
    pub(crate) fn verify(self, proof: RequestProof) -> Result<StoredReceipt, WriteError> {
        let (replay_key_id, digest) = proof.parts();
        if self.replay_key_id != replay_key_id
            || !self.digest.matches(&RegistrationRequestMac::new(digest))
        {
            return Err(ConsignmentError::CommandIdConflict.into());
        }
        Ok(self.receipt)
    }
}

/// 已完成公开头比较的已有命令，或仅限本事务的首次写入能力。
#[allow(dead_code, reason = "由后续任务 2D / 2F 消费仲裁结果")]
pub(crate) enum Arbitration<'tx, 'conn> {
    Replay(PendingReplay),
    First(CommandWriter<'tx, 'conn>),
}

/// 首次写入能力借用本次事务。事件在账本行写入前只在内存保存，挂单更新仍在原事务内。
pub(crate) struct CommandWriter<'tx, 'conn> {
    pub(super) tx: &'tx mut CommandTransaction<'conn>,
    pub(super) actor: Actor<'tx>,
    pub(super) operation: ListingOperation,
    pub(super) target: Target,
    pub(super) events: Vec<PendingEvent>,
}

impl<'conn> CommandTransaction<'conn> {
    /// 不变量 1、2、6：登记前或目标预读缺失时均先仲裁，不在这里判断归属、业务版本或开关。
    #[allow(dead_code, reason = "由后续任务 2D / 2F 调用")]
    pub(crate) async fn arbitrate<'tx>(
        &'tx mut self,
        actor: Actor<'tx>,
        operation: ListingOperation,
        target: Target,
    ) -> Result<Arbitration<'tx, 'conn>, WriteError> {
        let row = sqlx::query(
            "SELECT actor_kind, actor_ref, operation, request_target, request_digest, replay_key_id,
                    result_status, result_code, result_body
             FROM public.consignment_commands WHERE command_id = $1",
        )
        .bind(self.command_id)
        .fetch_optional(&mut *self.connection)
        .await?;
        let Some(row) = row else {
            return Ok(Arbitration::First(CommandWriter {
                tx: self,
                actor,
                operation,
                target,
                events: Vec::new(),
            }));
        };
        if row.try_get::<&str, _>("actor_kind")? != actor.kind()
            || row.try_get::<&str, _>("actor_ref")? != actor.reference()
            || row.try_get::<&str, _>("operation")? != operation.as_str()
            || row.try_get::<&str, _>("request_target")? != target.as_text()
        {
            return Err(ConsignmentError::CommandIdConflict.into());
        }
        let Ok(digest) = <[u8; 32]>::try_from(row.try_get::<&[u8], _>("request_digest")?) else {
            return Err(WriteError::invariant("consignment_command_request_digest"));
        };
        Ok(Arbitration::Replay(PendingReplay {
            replay_key_id: row.try_get("replay_key_id")?,
            digest: RegistrationRequestMac::new(digest),
            receipt: StoredReceipt {
                status: row.try_get("result_status")?,
                code: row.try_get("result_code")?,
                body: row.try_get("result_body")?,
            },
        }))
    }
}

impl CommandWriter<'_, '_> {
    /// 供首次业务检查及其它寄售 owner 同事务追加事实。禁止保存点、外部调用或自行提交。
    #[allow(dead_code, reason = "由后续任务 2D / 2F 调用")]
    pub(crate) fn connection(&mut self) -> &mut PgConnection {
        self.tx.connection
    }

    pub(super) async fn insert_receipt(
        &mut self,
        proof: RequestProof,
        listing_id: Option<Uuid>,
        result: &StoredReceipt,
        now: auction_time::Timestamp,
    ) -> Result<(), WriteError> {
        let (replay_key_id, digest) = proof.parts();
        // 不变量 1、3、7、8：只追加已仲裁的定案，键和 URL 不进入 SQL。
        sqlx::query(
            "INSERT INTO public.consignment_commands
                (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
                 replay_key_id, listing_id, result_status, result_code, result_body, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(self.tx.command_id)
        .bind(self.actor.kind())
        .bind(self.actor.reference())
        .bind(self.operation.as_str())
        .bind(self.target.as_text())
        .bind(digest.as_slice())
        .bind(replay_key_id)
        .bind(listing_id)
        .bind(result.status)
        .bind(&result.code)
        .bind(&result.body)
        .bind(now)
        .execute(&mut *self.tx.connection)
        .await?;
        Ok(())
    }
}
