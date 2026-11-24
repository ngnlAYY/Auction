//! 密钥轮换的分页位置、信封 CAS 与旧钥引用统计。

use std::fmt;

use auction_domain::DataKeyDomain;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use thiserror::Error;
use uuid::Uuid;

use crate::database::operations::audit_health::AuditWriteResult as _;

mod admin_commands;
mod stored_keys;
pub use admin_commands::{replace_admin_command_key_envelope, scan_admin_command_key_envelopes};
pub use stored_keys::stored_key_ids;

/// 单页扫描上限；离线维护命令逐页推进，避免一次性持有全部密文。
pub const KEY_ROTATION_PAGE_SIZE: i64 = 200;

/// 信封存储操作的安全分类。数据库错误详情可能含行内容，禁止跨此边界传播。
#[derive(Debug, Error)]
pub enum KeyRotationError {
    /// 无法读取待处理信封或引用。
    #[error("读取密钥信封失败")]
    Read,
    /// 写回失败，最后一次写入结果可能未知。
    #[error("替换密钥信封失败")]
    Replace,
    /// 未确认写回，且当前行不是已退休或已换到目标钥的记录。
    #[error("密钥信封已并发变更，请重新扫描")]
    Conflict,
    /// 审计追加失败。
    #[error("追加密钥轮换审计失败")]
    Audit,
    /// 该域没有数据库信封。
    #[error("该数据密钥域不支持数据库轮换")]
    UnsupportedDomain,
}

fn read_failed(_error: sqlx::Error) -> KeyRotationError {
    KeyRotationError::Read
}

fn replace_failed(_error: sqlx::Error) -> KeyRotationError {
    KeyRotationError::Replace
}

fn audit_failed(_error: sqlx::Error) -> KeyRotationError {
    KeyRotationError::Audit
}

/// 信封来源。装备域的顺序同时用于跨表分页。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeyEnvelopeSource {
    /// 拍卖装备信封。
    Equipment,
    /// 活动登记信封。
    Registration,
    /// 逻辑比较钥的包装。
    ReplayKey,
    /// 玩家登录信封。
    UserLogin,
    /// 外部凭据信封。
    Credential,
    /// 寄售预约码密文。
    ReservationSecret,
    /// 寄售受保护证据密文。
    EvidenceEnvelope,
    /// Admin 用户命令的响应信封。
    AdminCommand,
}

impl KeyEnvelopeSource {
    /// 轮换回执里的稳定来源名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Equipment => "equipment_secrets",
            Self::Registration => "consignment_registration_secrets",
            Self::ReplayKey => "consignment_replay_keys",
            Self::UserLogin => "user_login_key_envelopes",
            Self::Credential => "credential_keys",
            Self::ReservationSecret => "consignment_reservation_secrets",
            Self::EvidenceEnvelope => "mail_evidence_envelopes",
            Self::AdminCommand => "user_admin_commands",
        }
    }

    const fn primary_key(self) -> &'static str {
        match self {
            Self::Equipment => "equipment_id",
            Self::Registration => "listing_id",
            Self::ReplayKey | Self::Credential => "id",
            Self::ReservationSecret => "reservation_id",
            Self::EvidenceEnvelope => "id",
            Self::UserLogin => "user_id",
            Self::AdminCommand => "command_id",
        }
    }

    fn select(self) -> String {
        let (aad, retired) = match self {
            Self::Equipment => ("equipment_id", "false"),
            Self::Registration => ("aad_equipment_id", "retired_at IS NOT NULL"),
            Self::ReservationSecret | Self::EvidenceEnvelope => {
                ("NULL::uuid", "retired_at IS NOT NULL")
            }
            _ => ("NULL::uuid", "false"),
        };
        let (ciphertext, fingerprint) = if self == Self::ReplayKey {
            ("wrapped_key", "''::text")
        } else {
            ("ciphertext", "fingerprint")
        };
        let (kind, scope) = if self == Self::Credential {
            ("credential_kind", "scope")
        } else {
            ("NULL::text", "NULL::text")
        };
        let (generation, seller, purpose, aad_owner) = match self {
            Self::ReservationSecret => ("generation", "seller_user_id", "NULL::text", "NULL::text"),
            Self::EvidenceEnvelope => ("NULL::bigint", "NULL::uuid", "purpose", "aad_owner"),
            _ => ("NULL::bigint", "NULL::uuid", "NULL::text", "NULL::text"),
        };
        format!(
            "SELECT {pk} AS entity_id, {aad} AS equipment_aad, {retired} AS retired,
                    {kind} AS credential_kind, {scope} AS scope,
                    {generation} AS generation, {seller} AS seller_user_id,
                    {purpose} AS purpose, {aad_owner} AS aad_owner,
                    {ciphertext} AS ciphertext, nonce, encryption_key_id,
                    {fingerprint} AS fingerprint FROM public.{table}",
            pk = self.primary_key(),
            table = self.as_str(),
        )
    }
}

const EQUIPMENT_SOURCES: [KeyEnvelopeSource; 3] =
    [KeyEnvelopeSource::Equipment, KeyEnvelopeSource::Registration, KeyEnvelopeSource::ReplayKey];

const CREDENTIAL_SOURCES: [KeyEnvelopeSource; 3] = [
    KeyEnvelopeSource::Credential,
    KeyEnvelopeSource::ReservationSecret,
    KeyEnvelopeSource::EvidenceEnvelope,
];

/// 上一页最后一条信封的位置，表种与表内身份共同决定顺序。
#[derive(Clone, Copy)]
pub struct KeyEnvelopeCursor {
    source: KeyEnvelopeSource,
    entity_id: Uuid,
}

/// 存储中的一封密文信封；`credential_kind`/`scope` 只有凭据域使用。
#[derive(Clone)]
pub struct StoredKeyEnvelope {
    source: KeyEnvelopeSource,
    equipment_aad: Option<Uuid>,
    /// 当前来源表的主键。
    pub entity_id: Uuid,
    /// 登录信封或 Admin 命令响应所属的用户。
    pub owner_user_id: Option<Uuid>,
    /// 外部凭据的类型。
    pub credential_kind: Option<String>,
    /// 外部凭据的作用域。
    pub scope: Option<String>,
    /// 预约密文绑定的代。
    pub generation: Option<i64>,
    /// 预约密文绑定的卖家。
    pub seller_user_id: Option<Uuid>,
    /// 受保护证据的用途码。
    pub purpose: Option<String>,
    /// 受保护证据的 AAD owner。
    pub aad_owner: Option<String>,
    /// 密文及认证标签。
    pub ciphertext: Vec<u8>,
    /// 本次扫描捕获的 nonce。
    pub nonce: Vec<u8>,
    /// 信封引用的数据密钥标识。
    pub encryption_key_id: String,
    /// 普通信封的明文指纹。比较钥包装无此字段。
    pub fingerprint: String,
}

impl StoredKeyEnvelope {
    /// 装备域回执需初始化的来源，即使一处没有信封也记录零计数。
    pub const EQUIPMENT_SOURCES: [KeyEnvelopeSource; 3] = EQUIPMENT_SOURCES;

    /// 凭据域的三处来源；新增密文用途必须同时进入此清单。
    pub const CREDENTIAL_SOURCES: [KeyEnvelopeSource; 3] = CREDENTIAL_SOURCES;

    /// 供回执计数及写回选择正确的表。
    pub const fn source(&self) -> KeyEnvelopeSource {
        self.source
    }

    /// 以这条信封之后的位置继续扫描。
    pub const fn cursor(&self) -> KeyEnvelopeCursor {
        KeyEnvelopeCursor { source: self.source, entity_id: self.entity_id }
    }

    /// 装备及活动登记信封开封时使用的内部 AAD 身份。
    pub const fn equipment_aad(&self) -> Option<Uuid> {
        self.equipment_aad
    }

    /// 比较钥包装的逻辑身份。
    pub fn replay_key_id(&self) -> Option<Uuid> {
        (self.source == KeyEnvelopeSource::ReplayKey).then_some(self.entity_id)
    }

    /// 读取装备域的全部活动信封，供停机窗口内逐封复核当前钥。
    pub async fn scan_equipment_for_verification(
        pool: &PgPool,
        after: Option<KeyEnvelopeCursor>,
        limit: i64,
    ) -> Result<Vec<Self>, KeyRotationError> {
        scan_sources(pool, &EQUIPMENT_SOURCES, None, after, limit).await
    }

    /// 读取后清理信封缓冲，避免长期驻留。
    pub fn clear(&mut self) {
        self.ciphertext.fill(0);
        self.nonce.fill(0);
        self.fingerprint.clear();
    }
}

impl Drop for StoredKeyEnvelope {
    fn drop(&mut self) {
        self.clear();
    }
}

impl fmt::Debug for StoredKeyEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredKeyEnvelope")
            .field("source", &self.source.as_str())
            .finish_non_exhaustive()
    }
}

/// 重加密后的信封，退出作用域时清理密文缓冲。
pub struct RotatedKeyEnvelope {
    /// 新密文及认证标签。
    pub ciphertext: Vec<u8>,
    /// 新 nonce。
    pub nonce: Vec<u8>,
    /// 当前数据密钥标识。
    pub encryption_key_id: String,
    /// 普通信封的新指纹；比较钥包装不持久化指纹。
    pub fingerprint: String,
}

impl Drop for RotatedKeyEnvelope {
    fn drop(&mut self) {
        self.ciphertext.fill(0);
        self.nonce.fill(0);
        self.fingerprint.clear();
    }
}

/// 写回结果。并发换钥时提供当前信封供调用方重新认证。
#[derive(Debug)]
pub enum KeyEnvelopeReplacement {
    /// 本次 CAS 改写已提交。
    Replaced,
    /// 活动登记已被退休。
    Retired,
    /// 别的进程已换到本次目标钥，调用方仍须验证这份信封。
    AlreadyCurrent(Box<StoredKeyEnvelope>),
}

impl KeyEnvelopeReplacement {
    /// 本次是否写回。
    pub const fn was_replaced(&self) -> bool {
        matches!(self, Self::Replaced)
    }

    /// 并发进程写回的信封。
    pub fn current_envelope(&self) -> Option<&StoredKeyEnvelope> {
        match self {
            Self::AlreadyCurrent(row) => Some(row.as_ref()),
            Self::Replaced | Self::Retired => None,
        }
    }
}

/// 读取一页仍需重加密的信封，装备域逐表按主键独立续页。
pub async fn scan_key_envelopes(
    pool: &PgPool,
    domain: DataKeyDomain,
    current_key_id: &str,
    after: Option<KeyEnvelopeCursor>,
    limit: i64,
) -> Result<Vec<StoredKeyEnvelope>, KeyRotationError> {
    let sources: &[KeyEnvelopeSource] = match domain {
        DataKeyDomain::Equipment => &EQUIPMENT_SOURCES,
        DataKeyDomain::UserLogin => &[KeyEnvelopeSource::UserLogin],
        DataKeyDomain::Credential => &CREDENTIAL_SOURCES,
        DataKeyDomain::MailDiagnostics => return Err(KeyRotationError::UnsupportedDomain),
    };
    scan_sources(pool, sources, Some(current_key_id), after, limit).await
}

async fn scan_sources(
    pool: &PgPool,
    sources: &[KeyEnvelopeSource],
    excluded_key_id: Option<&str>,
    after: Option<KeyEnvelopeCursor>,
    limit: i64,
) -> Result<Vec<StoredKeyEnvelope>, KeyRotationError> {
    let mut remaining = limit.clamp(1, KEY_ROTATION_PAGE_SIZE);
    let mut page = Vec::new();
    for &source in sources {
        if after.is_some_and(|cursor| source < cursor.source) {
            continue;
        }
        let after_id =
            after.filter(|cursor| cursor.source == source).map(|cursor| cursor.entity_id);
        let active = if matches!(
            source,
            KeyEnvelopeSource::Registration
                | KeyEnvelopeSource::ReservationSecret
                | KeyEnvelopeSource::EvidenceEnvelope
        ) {
            "AND retired_at IS NULL"
        } else {
            ""
        };
        let pk = source.primary_key();
        let position = if after_id.is_some() { format!("AND {pk} > $3") } else { String::new() };
        // 表名、列名和 SQL 片段只来自闭集；密钥标识、主键和页长都绑定为参数。
        let sql = format!(
            "{} WHERE ($1::text IS NULL OR encryption_key_id <> $1) {position} {active}
                ORDER BY {pk} LIMIT $2",
            source.select(),
        );
        let query =
            sqlx::query(sqlx::AssertSqlSafe(sql.as_str())).bind(excluded_key_id).bind(remaining);
        let query = if let Some(after_id) = after_id { query.bind(after_id) } else { query };
        let rows = query.fetch_all(pool).await.map_err(read_failed)?;
        for row in rows {
            page.push(decode_envelope(source, &row)?);
            remaining -= 1;
        }
        if remaining == 0 {
            break;
        }
    }
    Ok(page)
}

fn decode_envelope(
    source: KeyEnvelopeSource,
    row: &PgRow,
) -> Result<StoredKeyEnvelope, KeyRotationError> {
    let entity_id = row.try_get("entity_id").map_err(read_failed)?;
    Ok(StoredKeyEnvelope {
        source,
        equipment_aad: row.try_get("equipment_aad").map_err(read_failed)?,
        entity_id,
        owner_user_id: (source == KeyEnvelopeSource::UserLogin).then_some(entity_id),
        credential_kind: row.try_get("credential_kind").map_err(read_failed)?,
        scope: row.try_get("scope").map_err(read_failed)?,
        generation: row.try_get("generation").map_err(read_failed)?,
        seller_user_id: row.try_get("seller_user_id").map_err(read_failed)?,
        purpose: row.try_get("purpose").map_err(read_failed)?,
        aad_owner: row.try_get("aad_owner").map_err(read_failed)?,
        ciphertext: row.try_get("ciphertext").map_err(read_failed)?,
        nonce: row.try_get("nonce").map_err(read_failed)?,
        encryption_key_id: row.try_get("encryption_key_id").map_err(read_failed)?,
        fingerprint: row.try_get("fingerprint").map_err(read_failed)?,
    })
}

/// 以扫描时的来源表、主键、数据钥 ID 和 nonce 为 CAS 条件替换。
pub async fn replace_key_envelope(
    pool: &PgPool,
    expected: &StoredKeyEnvelope,
    rotated: &RotatedKeyEnvelope,
) -> Result<KeyEnvelopeReplacement, KeyRotationError> {
    let source = expected.source;
    if source == KeyEnvelopeSource::AdminCommand {
        return Err(KeyRotationError::UnsupportedDomain);
    }
    let (payload, active) = match source {
        KeyEnvelopeSource::ReplayKey => ("wrapped_key = $1", ""),
        KeyEnvelopeSource::Registration
        | KeyEnvelopeSource::ReservationSecret
        | KeyEnvelopeSource::EvidenceEnvelope => {
            ("ciphertext = $1, fingerprint = $7", "AND retired_at IS NULL")
        }
        _ => ("ciphertext = $1, fingerprint = $7", ""),
    };
    // 不变量 1、5、6、7：只替换原信封的投影，旧领取或旧扫描不得覆盖更新、退休或命令事实。
    let sql = format!(
        "UPDATE public.{table}
         SET {payload}, nonce = $2, encryption_key_id = $3,
             updated_at = GREATEST(transaction_timestamp(), updated_at)
         WHERE {pk} = $4 AND encryption_key_id = $5 AND nonce = $6 {active}",
        table = source.as_str(),
        pk = source.primary_key(),
    );
    let query = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(&rotated.ciphertext)
        .bind(&rotated.nonce)
        .bind(&rotated.encryption_key_id)
        .bind(expected.entity_id)
        .bind(&expected.encryption_key_id)
        .bind(&expected.nonce);
    let query = if source == KeyEnvelopeSource::ReplayKey {
        query
    } else {
        query.bind(&rotated.fingerprint)
    };
    let result = query.execute(pool).await.map_err(replace_failed)?;
    if result.rows_affected() == 1 {
        return Ok(KeyEnvelopeReplacement::Replaced);
    }

    if !EQUIPMENT_SOURCES.contains(&source)
        && !matches!(
            source,
            KeyEnvelopeSource::ReservationSecret | KeyEnvelopeSource::EvidenceEnvelope
        )
    {
        return Err(KeyRotationError::Conflict);
    }
    recheck_equipment_envelope(pool, expected, &rotated.encryption_key_id).await
}

async fn recheck_equipment_envelope(
    pool: &PgPool,
    expected: &StoredKeyEnvelope,
    current_key_id: &str,
) -> Result<KeyEnvelopeReplacement, KeyRotationError> {
    let source = expected.source;
    // 独立语句取得 CAS 之后的新快照，避免用等待并发写入前的快照判断。
    let sql = format!("{} WHERE {} = $1", source.select(), source.primary_key());
    let row = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(expected.entity_id)
        .fetch_optional(pool)
        .await
        .map_err(read_failed)?
        .ok_or(KeyRotationError::Conflict)?;
    if row.try_get("retired").map_err(read_failed)? {
        return Ok(KeyEnvelopeReplacement::Retired);
    }
    let current = decode_envelope(source, &row)?;
    if current.encryption_key_id == current_key_id
        && current.equipment_aad == expected.equipment_aad
    {
        return Ok(KeyEnvelopeReplacement::AlreadyCurrent(Box::new(current)));
    }
    Err(KeyRotationError::Conflict)
}

/// 追加离线重加密审计；metadata 使用脱敏计数与回执身份。
pub async fn append_reencrypt_audit(
    pool: &PgPool,
    domain: DataKeyDomain,
    actor_id: &str,
    metadata: serde_json::Value,
) -> Result<(), KeyRotationError> {
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
         VALUES ($1, 'system', $2, 'system.data_key_reencrypt', 'data_key', $3,
                 'success', 'system', $4)",
    )
    .bind(Uuid::new_v4())
    .bind(actor_id)
    .bind(domain.as_str())
    .bind(metadata)
    .execute(pool)
    .await
    .record_audit_failure()
    .map_err(audit_failed)?;
    Ok(())
}
