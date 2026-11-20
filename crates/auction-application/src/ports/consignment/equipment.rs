//! 装备 URL 边界、白名单快照、解析分类与两类登记凭证。

use std::fmt;
use std::time::Duration;

use auction_domain::consignment::{ListingEquipmentIdentity, ListingReason};
use auction_domain::{
    EquipmentId, EquipmentKey, EquipmentKeyEnvelope, EquipmentKeyError, valid_key_id, valid_text,
};
use auction_time::Timestamp;
use subtle::ConstantTimeEq;
use thiserror::Error;
use uuid::Uuid;

use crate::ports::consignment::commands::RegistrationInput;
use crate::ports::consignment::errors::ConsignmentError;
use crate::ports::core::TransactionFuture;

/// 合同第 5 节固定的装备来源。
pub const LISTING_EQUIPMENT_SOURCE: &str = "hvdata-equip-v1";

/// 登记 URL 经规范化后唯一保留的身份与短生命周期凭证。
#[derive(Debug)]
pub struct ListingEquipmentReference {
    /// 规范世界和 EID。
    pub identity: ListingEquipmentIdentity,
    /// 装备读取凭证，调试输出脱敏且退出时尽力擦除。
    pub key: EquipmentKey,
}

/// 登记入口只在调用期间借用原 URL，复用既有装备 URL 规则并执行寄售的严格 EID 规则。
pub trait ListingEquipmentUrlValidator: Send + Sync {
    /// 解析规范身份及凭证；失败统一返回无 URL 原文的输入错误，不产生外部访问。
    fn validate(&self, equipment_url: &str) -> Result<ListingEquipmentReference, ConsignmentError>;
}

/// 安全装备等级。数字量在组成白名单装备信息时校验。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingEquipmentLevel {
    /// 已分配等级。
    Assigned(u32),
    /// 未分配等级。
    Unassigned,
    /// 魂绑且不可交易。
    Soulbound,
}

/// 装备快照允许采集的身份描述字段。
#[derive(Clone, PartialEq, Eq)]
pub struct ListingEquipmentLabel {
    /// 装备主名称。
    pub name: String,
    /// 可选装备别名。
    pub alt_name: Option<String>,
    /// 装备分类。
    pub category: String,
    /// 等级语义。
    pub level: ListingEquipmentLevel,
    /// 已确认的可交易性。
    pub tradeable: bool,
}

/// 一个白名单装备属性。
#[derive(Clone, PartialEq)]
pub struct ListingEquipmentStat {
    /// 属性名。
    pub name: String,
    /// 展示数值。
    pub value: f64,
    /// 来源可省略的基础数值。
    pub base: Option<f64>,
}

/// 白名单装备属性的一个分组。
#[derive(Clone, PartialEq)]
pub struct ListingEquipmentStatGroup {
    /// 分组名。
    pub name: String,
    /// 本组的属性。
    pub stats: Vec<ListingEquipmentStat>,
}

/// 可映射给浏览器的安全装备信息。不含第三方持有人、回显 key 或原始正文。
#[derive(Clone, PartialEq)]
pub struct ListingEquipmentDetails {
    observed_at: Timestamp,
    label: ListingEquipmentLabel,
    stat_groups: Vec<ListingEquipmentStatGroup>,
}

impl ListingEquipmentDetails {
    /// 验证已解码的白名单字段并按 UTF-8 名排序；传输 JSON 预算由适配器在解码时执行。
    pub fn new(
        observed_at: Timestamp,
        label: ListingEquipmentLabel,
        mut stat_groups: Vec<ListingEquipmentStatGroup>,
    ) -> Result<Self, LookupPermanentReason> {
        if !safe_text(&label.name, 512)
            || !safe_text(&label.category, 128)
            || label.alt_name.as_deref().is_some_and(|name| !safe_text(name, 512))
            || matches!(label.level, ListingEquipmentLevel::Assigned(n) if !(1..=100_000).contains(&n))
            || (label.level == ListingEquipmentLevel::Soulbound && label.tradeable)
            || stat_groups.len() > 128
        {
            return Err(LookupPermanentReason::SourceSchemaInvalid);
        }
        let mut count = 0;
        stat_groups.sort_by(|lhs, rhs| lhs.name.cmp(&rhs.name));
        if stat_groups.windows(2).any(|pair| pair[0].name == pair[1].name) {
            return Err(LookupPermanentReason::SourceSchemaInvalid);
        }
        for group in &mut stat_groups {
            if !safe_text(&group.name, 128) || group.stats.len() > 128 {
                return Err(LookupPermanentReason::SourceSchemaInvalid);
            }
            count += group.stats.len();
            group.stats.sort_by(|lhs, rhs| lhs.name.cmp(&rhs.name));
            if count > 256
                || group.stats.windows(2).any(|pair| pair[0].name == pair[1].name)
                || group.stats.iter().any(|stat| {
                    !safe_text(&stat.name, 128)
                        || !valid_stat(stat.value)
                        || stat.base.is_some_and(|base| !valid_stat(base))
                })
            {
                return Err(LookupPermanentReason::SourceSchemaInvalid);
            }
        }
        Ok(Self { observed_at, label, stat_groups })
    }

    /// 采集时刻，仅用于证据和展示，不参与重放摘要。
    pub const fn observed_at(&self) -> Timestamp {
        self.observed_at
    }

    /// 借用已验证的白名单描述。
    pub const fn label(&self) -> &ListingEquipmentLabel {
        &self.label
    }

    /// 借用按名称稳定排序的属性。
    pub fn stat_groups(&self) -> &[ListingEquipmentStatGroup] {
        &self.stat_groups
    }

    fn text_fields(&self) -> impl Iterator<Item = &str> {
        [self.label.name.as_str(), self.label.category.as_str()]
            .into_iter()
            .chain(self.label.alt_name.as_deref())
            .chain(self.stat_groups.iter().flat_map(|group| {
                std::iter::once(group.name.as_str())
                    .chain(group.stats.iter().map(|stat| stat.name.as_str()))
            }))
    }
}

impl fmt::Debug for ListingEquipmentDetails {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ListingEquipmentDetails")
            .field("observed_at", &self.observed_at)
            .finish_non_exhaustive()
    }
}

fn valid_stat(value: f64) -> bool {
    (-1e9..=1e9).contains(&value)
}

fn safe_text(value: &str, max: usize) -> bool {
    valid_text(value, max)
        && !value.contains(['\u{2028}', '\u{2029}'])
        && !contains_equipment_link(value)
}

pub(super) fn contains_equipment_link(value: &str) -> bool {
    [b"/equip/".as_slice(), b"/equip?".as_slice(), b"key=".as_slice()].into_iter().any(|fragment| {
        value.as_bytes().windows(fragment.len()).any(|part| part.eq_ignore_ascii_case(fragment))
    })
}

/// 可提交的完整观察，持有人仅用于锁内资格核验。
#[derive(Clone, PartialEq)]
pub struct ListingLookupSnapshot {
    identity: ListingEquipmentIdentity,
    details: ListingEquipmentDetails,
    owner_name: String,
}

impl ListingLookupSnapshot {
    /// 从已验证回显的请求身份建设快照，并拒绝消耗字段藏入本次 key 的响应。
    pub fn new(
        identity: ListingEquipmentIdentity,
        details: ListingEquipmentDetails,
        owner_name: String,
        key: &EquipmentKey,
    ) -> Result<Self, ListingEquipmentLookupError> {
        // 不变量 6、7：上游可能把本次凭证嵌入白名单文本；一律拒绝且不回显原值。
        if !safe_text(&owner_name, 256) {
            return Err(LookupPermanentReason::SourceSchemaInvalid.into());
        }
        let echoes_key = key
            .with_bytes(|key_bytes| {
                details.text_fields().chain(std::iter::once(owner_name.as_str())).any(|text| {
                    text.as_bytes()
                        .windows(key_bytes.len())
                        .any(|segment| segment.eq_ignore_ascii_case(key_bytes))
                })
            })
            .map_err(|error| match error {
                EquipmentKeyError::InvalidFormat => {
                    ListingEquipmentLookupError::CredentialUnavailable
                }
            })?;
        if echoes_key {
            return Err(LookupPermanentReason::SourceSchemaInvalid.into());
        }
        Ok(Self { identity, details, owner_name })
    }

    /// 本次请求的规范世界与 EID。
    pub const fn identity(&self) -> &ListingEquipmentIdentity {
        &self.identity
    }

    /// 可持久化并按读权限投影的装备详情。
    pub const fn details(&self) -> &ListingEquipmentDetails {
        &self.details
    }

    /// 仅供存储核对本地卖家身份，禁止进入读模型。
    pub fn owner_name(&self) -> &str {
        &self.owner_name
    }
}

impl fmt::Debug for ListingLookupSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ListingLookupSnapshot")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

/// 首次即终结且没有可信快照的上游分类。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum LookupPermanentReason {
    /// 装备或读取 key 未找到。
    #[error("来源未找到装备")]
    EquipmentNotFound,
    /// 已知拒绝请求响应。
    #[error("来源拒绝装备读取请求")]
    SourceRequestRejected,
    /// 状态码或 HTTP 响应不符合合同。
    #[error("装备来源 HTTP 响应无效")]
    SourceHttpInvalid,
    /// 正文或字段无效。
    #[error("装备来源字段无效")]
    SourceSchemaInvalid,
    /// 来源响应超出预算。
    #[error("装备来源响应超限")]
    SourceTooLarge,
    /// 来源成功字段的形态尚未确认。
    #[error("装备来源字段形态尚未确认")]
    SourceShapeUnconfirmed,
    /// 响应的世界、EID 或 key 与请求不符。
    #[error("装备来源身份不符")]
    SourceIdentityMismatch,
    /// 缺少持有人信息。
    #[error("装备来源缺少持有人")]
    OwnerMissing,
}

impl From<LookupPermanentReason> for ListingReason {
    fn from(reason: LookupPermanentReason) -> Self {
        match reason {
            LookupPermanentReason::EquipmentNotFound => Self::EquipmentNotFound,
            LookupPermanentReason::SourceRequestRejected => Self::SourceRequestRejected,
            LookupPermanentReason::SourceHttpInvalid => Self::SourceHttpInvalid,
            LookupPermanentReason::SourceSchemaInvalid => Self::SourceSchemaInvalid,
            LookupPermanentReason::SourceTooLarge => Self::SourceTooLarge,
            LookupPermanentReason::SourceShapeUnconfirmed => Self::SourceShapeUnconfirmed,
            LookupPermanentReason::SourceIdentityMismatch => Self::SourceIdentityMismatch,
            LookupPermanentReason::OwnerMissing => Self::OwnerMissing,
        }
    }
}

/// 只记工作进度的暂时分类，耗尽后挂单主原因使用 `lookup_exhausted`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupTransientReason {
    /// DNS、TLS、连接中断或 5xx。
    SourceUnavailable,
    /// 总预算已耗尽。
    SourceTimeout,
    /// 上游返回 429。
    SourceRateLimited,
}

impl LookupTransientReason {
    /// 返回安全进度码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourceUnavailable => "source_unavailable",
            Self::SourceTimeout => "source_timeout",
            Self::SourceRateLimited => "source_rate_limited",
        }
    }
}

/// 解析后的 429 `Retry-After` 提示，以最多六小时进入命令摘要及冷却决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LookupRetryAfter(Duration);

impl LookupRetryAfter {
    /// 接受已解析的非负时长，按合同取六小时上限。
    pub fn new(delay: Duration) -> Self {
        Self(delay.min(Duration::from_secs(6 * 3600)))
    }

    /// 读取受限后的提示时长；无效提示由调用方用 `None` 表示。
    pub const fn duration(self) -> Duration {
        self.0
    }
}

/// 第三方读取错误的闭集，不带响应正文、URL 或内部错误链。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ListingEquipmentLookupError {
    /// 不得重试的永久分类。
    #[error(transparent)]
    Permanent(#[from] LookupPermanentReason),
    /// DNS、TLS、连接中断或 5xx。
    #[error("装备来源暂不可用")]
    SourceUnavailable,
    /// 总预算耗尽。
    #[error("装备来源读取超时")]
    SourceTimeout,
    /// 上游 429，可携带经过裁剪的等待提示。
    #[error("装备来源正在限流")]
    SourceRateLimited(Option<LookupRetryAfter>),
    /// 本地凭证故障，应在发网前识别。
    #[error("登记凭证不可用")]
    CredentialUnavailable,
}

impl ListingEquipmentLookupError {
    /// 暂时分类供工作重排；永久分类不参与重排。
    pub const fn transient_reason(self) -> Option<LookupTransientReason> {
        match self {
            Self::Permanent(_) | Self::CredentialUnavailable => None,
            Self::SourceUnavailable => Some(LookupTransientReason::SourceUnavailable),
            Self::SourceTimeout => Some(LookupTransientReason::SourceTimeout),
            Self::SourceRateLimited(_) => Some(LookupTransientReason::SourceRateLimited),
        }
    }
}

/// 本地围栏事务提交的观察。凭证故障发生在出站之前。
#[derive(Debug)]
pub enum LookupObservation {
    /// 合规快照，待锁内核对资格、持有人和历史。
    Equipment(ListingLookupSnapshot),
    /// 来源读取失败或在出站前发现登记凭证不可用。
    Failed(ListingEquipmentLookupError),
}

/// Worker 使用的固定第三方来源，参数来自经核验的租约和短期密钥。
pub trait ListingEquipmentLookup: Send + Sync {
    /// 按身份内的世界和 EID 访问固定来源。截止覆盖 DNS、连接、读体与解码全程。
    fn lookup<'a>(
        &'a self,
        identity: &'a ListingEquipmentIdentity,
        key: &'a EquipmentKey,
        deadline: Timestamp,
    ) -> TransactionFuture<'a, ListingLookupSnapshot, ListingEquipmentLookupError>;
}

/// 挂单专用的在用登记信封，携带终身固定的认证身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingRegistrationSecret {
    aad_equipment_id: EquipmentId,
    envelope: EquipmentKeyEnvelope,
}

impl ListingRegistrationSecret {
    /// 复用既有信封类型并补寄售表的非零 AAD 和指纹约束。
    pub fn new(
        aad_equipment_id: EquipmentId,
        envelope: EquipmentKeyEnvelope,
    ) -> Result<Self, ListingCredentialError> {
        let fingerprint = envelope.fingerprint();
        if aad_equipment_id.is_nil()
            || fingerprint.len() != 64
            || !fingerprint.bytes().all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(ListingCredentialError);
        }
        Ok(Self { aad_equipment_id, envelope })
    }

    /// 供既有装备开封能力使用的 AAD 身份。
    pub const fn aad_equipment_id(&self) -> EquipmentId {
        self.aad_equipment_id
    }

    /// 借用活动信封。持久终结或发布时由存储擦除密文字段。
    pub const fn envelope(&self) -> &EquipmentKeyEnvelope {
        &self.envelope
    }
}

/// 凭证失败的安全投影，禁止保留底层密码错误链。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error("登记凭证不可用")]
pub struct ListingCredentialError;

impl From<ListingCredentialError> for ConsignmentError {
    fn from(_error: ListingCredentialError) -> Self {
        Self::Unavailable
    }
}

/// Worker 的登记开封边界。实现复用 integrations 中已有的 `EquipmentKeyOpener`。
pub trait ListingEquipmentKeyOpener: Send + Sync {
    /// 用原 AAD UUID 和对应 keyring 开封。失败不得发出来源请求。
    fn open(
        &self,
        secret: &ListingRegistrationSecret,
    ) -> Result<EquipmentKey, ListingCredentialError>;
}

/// 登记幂等比较所用的 HMAC-SHA-256 结果，调试输出隐去摘要。
#[derive(Clone, Copy)]
pub struct RegistrationRequestMac([u8; 32]);

impl RegistrationRequestMac {
    /// 从可信比较钥适配器或账本固定长度字段读入。
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// 供账本持久化。
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// 以常量时间比较两个摘要。
    pub fn matches(&self, other: &Self) -> bool {
        self.0.ct_eq(&other.0).into()
    }
}

impl fmt::Debug for RegistrationRequestMac {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 逻辑比较钥的密文，封装用途独立于装备读取凭证。
#[derive(Clone)]
pub struct WrappedRegistrationReplayKey {
    wrapped_key: Vec<u8>,
    nonce: [u8; 12],
    encryption_key_id: String,
}

impl WrappedRegistrationReplayKey {
    /// 根据 32 字节逻辑钥、既有 AEAD 的 16 字节标签和 keyring 的 ID 规则验证形状。
    pub fn new(
        wrapped_key: Vec<u8>,
        nonce: [u8; 12],
        encryption_key_id: String,
    ) -> Result<Self, ListingCredentialError> {
        if wrapped_key.len() != 48 || !valid_key_id(&encryption_key_id) {
            return Err(ListingCredentialError);
        }
        Ok(Self { wrapped_key, nonce, encryption_key_id })
    }

    /// 借用认证密文供开封或持久化使用。
    pub fn wrapped_key(&self) -> &[u8] {
        &self.wrapped_key
    }

    /// 借用 nonce 供开封、轮换 CAS 或持久化使用。
    pub const fn nonce(&self) -> &[u8; 12] {
        &self.nonce
    }

    /// 借用该信封引用的数据密钥 ID。
    pub fn encryption_key_id(&self) -> &str {
        &self.encryption_key_id
    }
}

impl fmt::Debug for WrappedRegistrationReplayKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 逻辑比较钥的本地密码能力。不变量 1、7：旧账本引用的比较钥必须可验证，明文钥不跨端口。
pub trait RegistrationReplayCrypto: Send + Sync {
    /// 生成 32 字节随机逻辑钥并用当前装备数据钥封装，独立 AAD 绑定用途、协议版本和逻辑 ID。
    fn generate_wrapped(
        &self,
        replay_key_id: Uuid,
    ) -> Result<WrappedRegistrationReplayKey, ListingCredentialError>;

    /// 验证仍可开封并使用此逻辑钥，用于登记就绪判断。明文钥仅在本地短暂存在。
    fn check_wrapped(
        &self,
        replay_key_id: Uuid,
        wrapped: &WrappedRegistrationReplayKey,
    ) -> Result<(), ListingCredentialError>;

    /// 用账本所绑定的逻辑钥对规范输入求 MAC，直接消费借用的 key。
    fn registration_mac(
        &self,
        replay_key_id: Uuid,
        wrapped: &WrappedRegistrationReplayKey,
        input: &RegistrationInput<'_>,
    ) -> Result<RegistrationRequestMac, ListingCredentialError>;

    /// 保持逻辑钥和 AAD 身份不变，改用当前装备数据钥封装；存储以原 key ID 和 nonce CAS。
    fn rewrap(
        &self,
        replay_key_id: Uuid,
        wrapped: &WrappedRegistrationReplayKey,
    ) -> Result<WrappedRegistrationReplayKey, ListingCredentialError>;
}
