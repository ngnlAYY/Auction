//! 全局命令账本、首写能力与安全回放。

use auction_application::consignment::{
    ConsignmentError, ListingCredentialError, ListingOperation, RegistrationRequestMac,
    WrappedRegistrationReplayKey,
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

/// 登记流程提供的本地 MAC 计算。底座只借出账本所指逻辑钥的包装项。
pub(crate) type RegistrationMac<'a> = dyn Fn(
        Uuid,
        &WrappedRegistrationReplayKey,
    ) -> Result<RegistrationRequestMac, ListingCredentialError>
    + Sync
    + 'a;

/// 应用层预计算的普通命令摘要，或登记重放所需的本地 MAC 计算。
#[allow(dead_code, reason = "由后续任务 2D / 2F 调用")]
#[derive(Clone, Copy)]
pub(crate) enum RequestProof<'a> {
    Digest(RequestHash),
    Registration(&'a RegistrationMac<'a>),
}

/// 首次登记由调用方以当前逻辑钥计算的 MAC，待与回执同事务存入。
pub(crate) struct RegistrationProof {
    /// 当前登记写入逻辑钥的稳定 ID。
    pub(crate) replay_key_id: Uuid,
    /// 调用方用该逻辑钥为本次登记生成的 MAC。
    pub(crate) mac: RegistrationRequestMac,
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

/// 已核实证明的已有命令，或仅限本事务的首次写入能力。
#[allow(dead_code, reason = "由后续任务 2D / 2F 消费仲裁结果")]
pub(crate) enum Arbitration<'tx, 'conn> {
    Replay(StoredReceipt),
    First(CommandWriter<'tx, 'conn>),
}

/// 首次写入能力借用本次事务。事件在账本行写入前只在内存保存，挂单更新仍在原事务内。
pub(crate) struct CommandWriter<'tx, 'conn> {
    pub(super) tx: &'tx mut CommandTransaction<'conn>,
    pub(super) actor: Actor<'tx>,
    pub(super) operation: ListingOperation,
    pub(super) target: Target,
    pub(super) events: Vec<PendingEvent>,
    request_hash: Option<RequestHash>,
}

impl<'conn> CommandTransaction<'conn> {
    /// 不变量 1、2、6：登记前或目标预读缺失时均先仲裁，不在这里判断归属、业务版本或开关。
    /// 登记的 MAC 回调仅在已有账本行的身份匹配后执行，必须为有界的本地计算。
    #[allow(dead_code, reason = "由后续任务 2D / 2F 调用")]
    pub(crate) async fn arbitrate<'tx>(
        &'tx mut self,
        actor: Actor<'tx>,
        operation: ListingOperation,
        target: Target,
        proof: RequestProof<'_>,
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
            let request_hash = match (operation, proof) {
                (ListingOperation::Register, RequestProof::Registration(_)) => None,
                (ListingOperation::Register, RequestProof::Digest(_))
                | (_, RequestProof::Registration(_)) => {
                    return Err(WriteError::invariant("consignment_command_proof_kind"));
                }
                (_, RequestProof::Digest(hash)) => Some(hash),
            };
            return Ok(Arbitration::First(CommandWriter {
                tx: self,
                actor,
                operation,
                target,
                events: Vec::new(),
                request_hash,
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
        let replay_key_id: Option<Uuid> = row.try_get("replay_key_id")?;
        let provided = match (replay_key_id, proof) {
            (None, RequestProof::Digest(hash)) => RegistrationRequestMac::new(hash.as_bytes()),
            (Some(replay_key_id), RequestProof::Registration(compute_mac)) => {
                // 不变量 1、7：先核对账本身份，仅取该行指向的包装项；旧命令不随写钥轮换。
                let wrapped = load_replay_key(self.connection, replay_key_id).await?;
                compute_mac(replay_key_id, &wrapped).map_err(ConsignmentError::from)?
            }
            _ => return Err(ConsignmentError::CommandIdConflict.into()),
        };
        if !RegistrationRequestMac::new(digest).matches(&provided) {
            return Err(ConsignmentError::CommandIdConflict.into());
        }
        Ok(Arbitration::Replay(StoredReceipt {
            status: row.try_get("result_status")?,
            code: row.try_get("result_code")?,
            body: row.try_get("result_body")?,
        }))
    }
}

async fn load_replay_key(
    connection: &mut PgConnection,
    replay_key_id: Uuid,
) -> Result<WrappedRegistrationReplayKey, WriteError> {
    let row = sqlx::query(
        "SELECT wrapped_key, nonce, encryption_key_id
         FROM public.consignment_replay_keys WHERE id = $1",
    )
    .bind(replay_key_id)
    .fetch_one(connection)
    .await?;
    let nonce: &[u8] = row.try_get("nonce")?;
    let Ok(nonce) = <[u8; 12]>::try_from(nonce) else {
        return Err(WriteError::invariant("consignment_replay_key_nonce"));
    };
    Ok(WrappedRegistrationReplayKey::new(
        row.try_get("wrapped_key")?,
        nonce,
        row.try_get("encryption_key_id")?,
    )
    .map_err(ConsignmentError::from)?)
}

impl CommandWriter<'_, '_> {
    /// 供首次业务检查及其它寄售 owner 同事务追加事实。禁止保存点、外部调用或自行提交。
    #[allow(dead_code, reason = "由后续任务 2D / 2F 调用")]
    pub(crate) fn connection(&mut self) -> &mut PgConnection {
        self.tx.connection
    }

    pub(super) async fn insert_receipt(
        &mut self,
        registration: Option<RegistrationProof>,
        listing_id: Option<Uuid>,
        result: &StoredReceipt,
        now: auction_time::Timestamp,
    ) -> Result<(), WriteError> {
        let (replay_key_id, digest) = match (self.request_hash, registration) {
            (Some(hash), None) => (None, hash.as_bytes()),
            (None, Some(proof)) => (Some(proof.replay_key_id), *proof.mac.as_bytes()),
            _ => return Err(WriteError::invariant("consignment_command_proof_kind")),
        };
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
