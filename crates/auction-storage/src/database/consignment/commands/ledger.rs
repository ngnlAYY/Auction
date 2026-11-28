//! 全局命令账本、首写能力与安全回放。

use auction_application::consignment::{
    ConsignmentError, DepositActor, DepositOperation, ListingCredentialError, ListingOperation,
    RegistrationRequestMac, WrappedRegistrationReplayKey,
};
use auction_application::core::RequestHash;
use auction_domain::consignment::OriginalMailKey;
use serde_json::Value;
use sqlx::{PgConnection, Row};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::facts::PendingEvent;
use crate::database::consignment::commands::session::{CommandTransaction, DepositTransaction};
use crate::database::consignment::commands::writer_v2::DepositWriter;

/// 仅由受信会话或本地装配提供的演员身份。Worker 应使用固定服务身份。
#[derive(Clone, Copy)]
pub(crate) enum Actor<'a> {
    PublicUser(Uuid),
    Admin(&'a str),
    Worker,
    System,
    /// 受限权利流程里的原浏览器（J5）；引用是浏览器绑定摘要的小写十六进制。
    Browser(&'a [u8; 32]),
    /// 已验证的受限权利人（J5）；引用是 `right_id`。
    RightHolder(Uuid),
    /// 未登录登记人（000138）；引用是原样自填游戏名，只能登记非寄售。
    Anonymous(&'a str),
}

impl Actor<'_> {
    pub(super) const fn kind(self) -> &'static str {
        match self {
            Self::PublicUser(_) => "public_user",
            Self::Admin(_) => "admin",
            Self::Worker => "worker",
            Self::System => "system",
            // 账本 `actor_kind` 的 CHECK 由第 7 期 7M-2 追加这两种；之前数据库约束拒绝，失败关闭。
            Self::Browser(_) => "browser",
            Self::RightHolder(_) => "right_holder",
            Self::Anonymous(_) => "anonymous",
        }
    }

    pub(super) fn reference(self) -> String {
        match self {
            Self::PublicUser(seller_user_id) => seller_user_id.to_string(),
            Self::Admin(actor_ref) => actor_ref.to_owned(),
            Self::Worker | Self::System => "consignment".into(),
            Self::Browser(binding_digest) => lower_hex(binding_digest),
            Self::RightHolder(right_id) => right_id.to_string(),
            Self::Anonymous(seller_game_name) => seller_game_name.to_owned(),
        }
    }

    pub(super) const fn audit_source(self) -> &'static str {
        match self {
            Self::PublicUser(_) | Self::Anonymous(_) => "public",
            _ => self.kind(),
        }
    }

    /// 审计 `actor_type` 闭集没有匿名种类；匿名登记人按 Public 来源的公开用户记账。
    pub(super) const fn audit_kind(self) -> &'static str {
        match self {
            Self::Anonymous(_) => "public_user",
            _ => self.kind(),
        }
    }

    /// 匿名审计引用带固定前缀，不会与账户 UUID 混淆。
    pub(super) fn audit_reference(self) -> String {
        match self {
            Self::Anonymous(seller_game_name) => format!("anonymous:{seller_game_name}"),
            _ => self.reference(),
        }
    }
}

/// 登记的固定目标 new 或已存在/待核的挂单 UUID。
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

/// v2 命令的类型化目标；与 `000134` 的 `target_kind` 列逐项对应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DepositTarget {
    /// 挂单目标。
    Listing(Uuid),
    /// 预约目标。
    Reservation(Uuid),
    /// 隔离目标。
    Quarantine(Uuid),
    /// 最终决定目标。
    FinalDecision(Uuid),
    /// 原起写 attempt 目标。
    OriginalAttempt(Uuid),
    /// 稳定原信键 K。
    MailKey(OriginalMailKey),
    /// 代管 position 目标（第 7 期灭失命令：确认、证据、人工结果）。
    Position(Uuid),
    /// 冻结季目标。
    Season {
        /// 季身份文本。
        season_id: String,
        /// 冻结日历版本摘要。
        calendar_version: [u8; 32],
    },
}

impl DepositTarget {
    /// 目标必须与 4A 的操作闭集一致；不匹配时不得写入账本。
    pub(crate) fn valid_for(&self, operation: DepositOperation) -> bool {
        matches!(
            (self, operation),
            (Self::Listing(_), DepositOperation::IssueReservation)
                | (Self::Listing(_), DepositOperation::Withdraw)
                | (Self::Listing(_), DepositOperation::AdminDelist)
                | (Self::Listing(_), DepositOperation::ReleaseOperationsHold)
                | (Self::Listing(_), DepositOperation::Relist)
                | (Self::Listing(_), DepositOperation::FulfillWithdrawIntent)
                | (Self::Position(_), DepositOperation::ConfirmPositionLoss)
                | (Self::Position(_), DepositOperation::RecordLossCaseOutcome)
                | (Self::Position(_), DepositOperation::RecordPositionLossProof)
                | (Self::Reservation(_), DepositOperation::ExpireReservation)
                | (Self::Quarantine(_), DepositOperation::AppendQuarantineObservation)
                | (Self::Quarantine(_), DepositOperation::RecheckQuarantine)
                | (Self::Quarantine(_), DepositOperation::ReleaseQuarantine)
                | (Self::FinalDecision(_), DepositOperation::StartDispositionWrite)
                | (Self::OriginalAttempt(_), DepositOperation::ApplyMailProof)
                | (Self::MailKey(_), DepositOperation::DecideEnvelope)
                | (Self::Season { .. }, DepositOperation::SeasonNotice)
                | (Self::Season { .. }, DepositOperation::SeasonClose)
        )
    }

    pub(crate) fn request_target(&self) -> String {
        match self {
            Self::Listing(id)
            | Self::Reservation(id)
            | Self::Quarantine(id)
            | Self::FinalDecision(id)
            | Self::OriginalAttempt(id)
            | Self::Position(id) => id.to_string(),
            Self::MailKey(key) => format!(
                "{}|{}|{}",
                key.authenticated_bot_uid(),
                auction_domain::consignment::settlement_world_code(key.world()),
                key.external_mid()
            ),
            Self::Season { season_id, .. } => season_id.clone(),
        }
    }

    #[allow(clippy::type_complexity)]
    pub(super) fn columns(
        &self,
    ) -> (
        &'static str,
        Option<Uuid>,
        Option<&str>,
        Option<&str>,
        Option<&str>,
        Option<&str>,
        Option<&[u8]>,
    ) {
        match self {
            Self::Listing(id) => ("listing", Some(*id), None, None, None, None, None),
            Self::Reservation(id) => ("reservation", Some(*id), None, None, None, None, None),
            Self::Quarantine(id) => ("quarantine", Some(*id), None, None, None, None, None),
            Self::FinalDecision(id) => ("final_decision", Some(*id), None, None, None, None, None),
            Self::OriginalAttempt(id) => {
                ("original_attempt", Some(*id), None, None, None, None, None)
            }
            Self::Position(id) => ("position", Some(*id), None, None, None, None, None),
            Self::MailKey(key) => (
                "mail_key",
                None,
                Some(key.authenticated_bot_uid()),
                Some(auction_domain::consignment::settlement_world_code(key.world())),
                Some(key.external_mid()),
                None,
                None,
            ),
            Self::Season { season_id, calendar_version } => {
                ("season", None, None, None, None, Some(season_id), Some(calendar_version))
            }
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
    pub(super) request_id: Option<&'tx str>,
}

impl<'conn> CommandTransaction<'conn> {
    /// 不变量 1、2、6：登记前或目标预读缺失时均先仲裁，不在这里判断归属、业务版本或开关。
    /// 登记的 MAC 回调仅在已有账本行的身份匹配后执行，必须为有界的本地计算。
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
                request_id: None,
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

/// v2 账本仲裁结果：已有回执原样重放，或授予仅限本事务的首次写入能力。
pub(crate) enum DepositArbitration<'w, 'tx, 'conn> {
    /// 同命令 ID、同请求的首次回执。
    Replay(StoredReceipt),
    /// 本事务首次写入。
    First(DepositWriter<'w, 'tx, 'conn>),
}

impl<'tx, 'conn> DepositTransaction<'tx, 'conn> {
    /// 不变量 1、2、6：先核 actor、operation、target 与摘要，再由调用方判开关、资格和版本。
    pub(crate) async fn arbitrate<'w>(
        &'w mut self,
        actor: DepositActor,
        operation: DepositOperation,
        target: DepositTarget,
        request_hash: RequestHash,
        request_id: Option<&'w str>,
    ) -> Result<DepositArbitration<'w, 'tx, 'conn>, WriteError> {
        if !target.valid_for(operation) {
            return Err(WriteError::invariant("consignment_deposit_command_target"));
        }
        let row = sqlx::query(
            "SELECT schema_version, actor_kind, actor_ref, operation, request_target,
                    request_digest, target_kind, target_id, target_bot_uid, target_world_id,
                    target_external_mid, target_season_id, target_calendar_version,
                    result_status, result_code, result_body
             FROM public.consignment_commands WHERE command_id = $1",
        )
        .bind(self.command_id)
        .fetch_optional(self.connection())
        .await?;
        let Some(row) = row else {
            return Ok(DepositArbitration::First(DepositWriter {
                tx: self,
                actor,
                operation,
                target,
                events: Vec::new(),
                request_hash,
                request_id,
                audit_note: None,
            }));
        };
        let schema_version: i16 = row.try_get("schema_version")?;
        let digest: &[u8] = row.try_get("request_digest")?;
        let same_target = match &target {
            DepositTarget::Listing(id)
            | DepositTarget::Reservation(id)
            | DepositTarget::Quarantine(id)
            | DepositTarget::FinalDecision(id)
            | DepositTarget::OriginalAttempt(id)
            | DepositTarget::Position(id) => {
                row.try_get::<Option<&str>, _>("target_kind")? == Some(target.kind())
                    && row.try_get::<Option<Uuid>, _>("target_id")? == Some(*id)
            }
            DepositTarget::MailKey(key) => {
                row.try_get::<Option<&str>, _>("target_kind")? == Some("mail_key")
                    && row.try_get::<Option<&str>, _>("target_bot_uid")?
                        == Some(key.authenticated_bot_uid())
                    && row.try_get::<Option<&str>, _>("target_world_id")?
                        == Some(auction_domain::consignment::settlement_world_code(key.world()))
                    && row.try_get::<Option<&str>, _>("target_external_mid")?
                        == Some(key.external_mid())
            }
            DepositTarget::Season { season_id, calendar_version } => {
                row.try_get::<Option<&str>, _>("target_kind")? == Some("season")
                    && row.try_get::<Option<&str>, _>("target_season_id")? == Some(season_id)
                    && row.try_get::<Option<&[u8]>, _>("target_calendar_version")?
                        == Some(calendar_version.as_slice())
            }
        };
        let digest_matches =
            digest.len() == 32 && bool::from(digest.ct_eq(request_hash.as_bytes().as_slice()));
        if schema_version != 2
            || row.try_get::<&str, _>("actor_kind")? != actor.kind()
            || row.try_get::<&str, _>("actor_ref")? != actor_reference(&actor)
            || row.try_get::<&str, _>("operation")? != operation.as_str()
            || row.try_get::<&str, _>("request_target")? != target.request_target()
            || !same_target
            || !digest_matches
        {
            return Err(ConsignmentError::CommandIdConflict.into());
        }
        Ok(DepositArbitration::Replay(StoredReceipt {
            status: row.try_get("result_status")?,
            code: row.try_get("result_code")?,
            body: row.try_get("result_body")?,
        }))
    }
}

pub(crate) fn actor_reference(actor: &DepositActor) -> String {
    match actor {
        DepositActor::PublicUser(user_id) => user_id.as_uuid().to_string(),
        DepositActor::Admin(actor) => actor.clone(),
        DepositActor::Worker | DepositActor::System => "consignment".into(),
        DepositActor::Browser { binding_digest } => lower_hex(binding_digest),
        DepositActor::RightHolder { right_id } => right_id.to_string(),
    }
}

/// 32 字节摘要的小写十六进制，与应用层规范编码的浏览器引用一致。
fn lower_hex(bytes: &[u8; 32]) -> String {
    let mut text = String::with_capacity(64);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

impl DepositTarget {
    pub(crate) const fn kind(&self) -> &'static str {
        match self {
            Self::Listing(_) => "listing",
            Self::Reservation(_) => "reservation",
            Self::Quarantine(_) => "quarantine",
            Self::FinalDecision(_) => "final_decision",
            Self::OriginalAttempt(_) => "original_attempt",
            Self::Position(_) => "position",
            Self::MailKey(_) => "mail_key",
            Self::Season { .. } => "season",
        }
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
