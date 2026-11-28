//! 认证 Bot 在原世界读取装备详情；第三方 hvdata 样本只做预检，不授予资产权。
//!
//! 本观察绑定同一认证 UID、凭据 ID/版本、runtime 代际与世界；Soulbound 负例保留为
//! `tradeable=false`，不能由第三方来源覆盖。

use std::time::Instant;

use auction_application::mail::MailEquipmentAttachment;
use auction_application::mail::MailEquipmentDiagnosticCategory;
use auction_application::settlement::MailAccountEvidenceBinding;
use auction_domain::SettlementWorld;
use auction_time::Timestamp;
use sha2::{Digest, Sha256};
use tokio::time::timeout_at;

use super::super::parser::parse_equipment_page;
use super::http::EquipmentFetchFailure;
use super::{EquipmentFetchError, EquipmentSourceAdapter};
use crate::clients::mooglemail::MoogleMailParser;
use crate::clients::mooglemail::{MoogleMailOperation, MoogleMailWorld};

/// 收件资格 / 取得的认证装备观察。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyEquipmentObservation {
    /// 同一认证会话绑定。
    pub binding: MailAccountEvidenceBinding,
    /// 读取世界。
    pub world: SettlementWorld,
    /// 外部装备编号。
    pub external_equipment_id: String,
    /// 装备名。
    pub name: String,
    /// 分类。
    pub category: String,
    /// 等级；Soulbound tier 与未分配为空。
    pub level: Option<i32>,
    /// 服务端声明的可交易性。
    pub tradeable: bool,
    /// 观察时刻。
    pub observed_at: Timestamp,
    /// 不含装备 key 与正文的摘要。
    pub digest: [u8; 32],
}

impl CustodyEquipmentObservation {
    /// 重新计算摘要；只覆盖公开装备事实与认证绑定。
    pub fn compute_digest(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(b"auction/consignment/custody-equipment/v1\0");
        digest.update(self.binding.digest);
        digest.update(self.world.as_str().as_bytes());
        digest.update(self.external_equipment_id.as_bytes());
        digest.update(u64::try_from(self.name.len()).unwrap_or(u64::MAX).to_be_bytes());
        digest.update(self.name.as_bytes());
        digest.update(self.category.as_bytes());
        // 未分配/魂绑与 Assigned(0) 必须可区分；编码只在本进程重算，不进入持久化格式。
        match self.level {
            Some(level) => {
                digest.update([1_u8]);
                digest.update(level.to_be_bytes());
            }
            None => {
                digest.update([0_u8]);
            }
        }
        digest.update([u8::from(self.tradeable)]);
        digest.update(self.observed_at.to_rfc3339().as_bytes());
        digest.finalize().into()
    }
}

/// 认证装备读取失败的闭集；临时状态不能退件或授予资格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustodyEquipmentError {
    /// 绑定、世界或装备编号无效。
    InvalidRequest,
    /// 当前凭据不能证明同一认证账号。
    IdentityUnavailable,
    /// 远端明确登录失效。
    SessionExpired,
    /// 战斗、限流或账号状态暂时不可用。
    TemporaryUnavailable,
    /// 网络、超时或远端 5xx。
    SourceUnavailable,
    /// 页面结构或装备身份不一致。
    ProtocolDrift,
}

impl CustodyEquipmentError {
    /// 稳定安全分类。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::IdentityUnavailable => "identity_unavailable",
            Self::SessionExpired => "session_expired",
            Self::TemporaryUnavailable => "temporary_unavailable",
            Self::SourceUnavailable => "source_unavailable",
            Self::ProtocolDrift => "protocol_drift",
        }
    }
}

impl EquipmentSourceAdapter {
    /// 认证 Bot 在原世界读取装备详情；要求与观察绑定相同的持久凭据与 runtime 代际。
    pub async fn read_custody_equipment(
        &self,
        binding: &MailAccountEvidenceBinding,
        attachment: MailEquipmentAttachment,
        deadline: Instant,
    ) -> Result<CustodyEquipmentObservation, CustodyEquipmentError> {
        if !binding.validate() || deadline <= Instant::now() {
            return Err(CustodyEquipmentError::InvalidRequest);
        }
        let world = MoogleMailWorld::from(binding.world);
        let Some(credentials) = self.credentials.as_ref() else {
            return Err(CustodyEquipmentError::IdentityUnavailable);
        };
        let session = credentials
            .begin_exclusive(world, MoogleMailOperation::Equipment)
            .await
            .map_err(|error| map_mail_error(error.category))?;
        let (credential_id, credential_version, runtime, expected) =
            session.internal_credential_binding(world).map_err(map_identity_error)?;
        let Some(expected_name) = expected else {
            // 没有持久玩家名就无法把本次会话绑定到观察里的认证 UID，按身份不可证明失败关闭。
            return Err(CustodyEquipmentError::IdentityUnavailable);
        };
        if credential_id != binding.credential_id
            || credential_version != binding.credential_version
            || runtime != binding.runtime
            || expected_name != binding.player_name
        {
            return Err(CustodyEquipmentError::IdentityUnavailable);
        }
        let mail_client =
            self.mail_client.as_ref().ok_or(CustodyEquipmentError::IdentityUnavailable)?;
        let identity_target = mail_client.identity(world);
        let identity_document = timeout_at(
            tokio::time::Instant::from_std(deadline),
            mail_client.get(&identity_target, session.cookie()),
        )
        .await
        .map_err(read_timeout)?
        .map_err(|error| map_mail_error(error.category))?;
        let session_uid = MoogleMailParser
            .parse_authenticated_uid(&identity_document, world)
            .map_err(|error| map_mail_error(error.category))?;
        // 凭据 ID/版本与 runtime 相同仍不足以证明是同一 UID；必须在本次会话内复核服务端声明的 UID。
        if session_uid != binding.authenticated_uid {
            return Err(CustodyEquipmentError::IdentityUnavailable);
        }
        let key = attachment
            .key
            .with_bytes(|bytes| std::str::from_utf8(bytes).map(str::to_owned))
            .map_err(invalid_key_material)?
            .map_err(invalid_key_text)?;
        // 装备编号必须先通过本地规范十进制校验再拼路径；否则 `/ ? # %` 可改变请求目标。
        // 与后续 canonical_eid 同口径（允许前导零，但必须含非零数字且不超 32 位）。
        if canonical_eid(&attachment.external_equipment_id).is_none() {
            return Err(CustodyEquipmentError::InvalidRequest);
        }
        let prefix = if world == MoogleMailWorld::Isekai { "/isekai/equip/" } else { "/equip/" };
        let mut target = self.endpoint.clone();
        target.set_path(&format!("{prefix}{}/{}", attachment.external_equipment_id, key));
        self.await_request_slot().await;
        let result = timeout_at(
            tokio::time::Instant::from_std(deadline),
            self.fetch_document_once_observed(world, target, Some(session.cookie()), None),
        )
        .await
        .map_err(read_timeout)?;
        let fetched = match result {
            Ok(fetched) => fetched,
            Err(EquipmentFetchError::Observed(failure)) => return Err(map_fetch_failure(failure)),
            Err(EquipmentFetchError::SessionExpired) => {
                return Err(CustodyEquipmentError::SessionExpired);
            }
            Err(EquipmentFetchError::Intake(_error)) => {
                return Err(CustodyEquipmentError::TemporaryUnavailable);
            }
        };
        session.check_current().await.map_err(map_current_error)?;
        let observed_at = Timestamp::now();
        let facts = parse_equipment_page(&fetched.body, &attachment.external_equipment_id)
            .map_err(equipment_page_drift)?;
        custody_observation_from_facts(binding, facts, observed_at)
    }
}

fn custody_observation_from_facts(
    binding: &MailAccountEvidenceBinding,
    facts: super::super::model::EquipmentFacts,
    observed_at: Timestamp,
) -> Result<CustodyEquipmentObservation, CustodyEquipmentError> {
    let external_equipment_id =
        canonical_eid(&facts.external_equipment_id).ok_or(CustodyEquipmentError::ProtocolDrift)?;
    if !auction_domain::valid_text(&facts.name, 512)
        || !auction_domain::valid_text(&facts.category, 128)
    {
        return Err(CustodyEquipmentError::ProtocolDrift);
    }
    let mut observation = CustodyEquipmentObservation {
        binding: binding.clone(),
        world: binding.world,
        external_equipment_id,
        name: facts.name,
        category: facts.category,
        level: facts.level.legacy_level(),
        tradeable: facts.tradeable,
        observed_at,
        digest: [0; 32],
    };
    observation.digest = observation.compute_digest();
    Ok(observation)
}

fn map_fetch_failure(failure: EquipmentFetchFailure) -> CustodyEquipmentError {
    match failure.category {
        MailEquipmentDiagnosticCategory::SessionExpired => CustodyEquipmentError::SessionExpired,
        MailEquipmentDiagnosticCategory::RateLimited
        | MailEquipmentDiagnosticCategory::CredentialUnavailable
        | MailEquipmentDiagnosticCategory::Timeout
        | MailEquipmentDiagnosticCategory::Network
        | MailEquipmentDiagnosticCategory::ServerError => {
            CustodyEquipmentError::TemporaryUnavailable
        }
        _ => CustodyEquipmentError::ProtocolDrift,
    }
}

fn map_mail_error(
    category: crate::clients::mooglemail::MoogleMailErrorCategory,
) -> CustodyEquipmentError {
    use crate::clients::mooglemail::MoogleMailErrorCategory;
    match category {
        MoogleMailErrorCategory::SessionExpired => CustodyEquipmentError::SessionExpired,
        MoogleMailErrorCategory::RateLimited
        | MoogleMailErrorCategory::AccountBusy
        | MoogleMailErrorCategory::RemoteTemporary
        | MoogleMailErrorCategory::CredentialUnavailable => {
            CustodyEquipmentError::TemporaryUnavailable
        }
        _ => CustodyEquipmentError::IdentityUnavailable,
    }
}

fn map_identity_error(
    error: auction_application::settlement::InternalEvidenceError,
) -> CustodyEquipmentError {
    use auction_application::settlement::InternalEvidenceError;
    match error {
        InternalEvidenceError::Unavailable
        | InternalEvidenceError::Expired
        | InternalEvidenceError::Revoked
        | InternalEvidenceError::Unsupported
        | InternalEvidenceError::Unproven
        | InternalEvidenceError::InvalidRequest => CustodyEquipmentError::IdentityUnavailable,
    }
}

fn map_current_error(error: crate::clients::mooglemail::MoogleMailError) -> CustodyEquipmentError {
    map_mail_error(error.category)
}

fn invalid_key_material(_: auction_domain::EquipmentKeyError) -> CustodyEquipmentError {
    CustodyEquipmentError::InvalidRequest
}

fn invalid_key_text(_: std::str::Utf8Error) -> CustodyEquipmentError {
    CustodyEquipmentError::InvalidRequest
}

fn read_timeout(_: tokio::time::error::Elapsed) -> CustodyEquipmentError {
    CustodyEquipmentError::SourceUnavailable
}

fn equipment_page_drift(
    _: auction_application::mail::EquipmentSourceDetailCode,
) -> CustodyEquipmentError {
    CustodyEquipmentError::ProtocolDrift
}

fn canonical_eid(raw: &str) -> Option<String> {
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let canonical = raw.trim_start_matches('0');
    (!canonical.is_empty() && canonical.len() <= 32).then(|| canonical.to_owned())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use auction_application::admin::{
        AdminCredentialRuntimeOwnership, AdminCredentialRuntimeWriter,
    };
    use auction_application::core::ApplicationFuture;
    use auction_application::mail::MailEquipmentAttachment;
    use auction_application::settlement::{MailAccountEvidenceBinding, MailRuntimeBinding};
    use auction_domain::EquipmentKey;
    use auction_domain::SettlementWorld;
    use auction_test_support::mooglemail::MailDouble;
    use auction_time::Timestamp;
    use tokio::sync::Mutex;
    use uuid::Uuid;

    use super::{CustodyEquipmentError, custody_observation_from_facts};
    use crate::clients::mooglemail::{
        MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie, MoogleMailCredentialSet,
        MoogleMailRuntimeCredential, MoogleMailWorld,
    };
    use crate::equipment::config::EquipmentSourcePolicy;
    use crate::equipment::model::EquipmentFacts;
    use crate::equipment::parser::EquipmentLevel;
    use crate::equipment::reader::{EquipmentSourceAdapter, build_equipment_http_client};

    const COOKIE: &str = "ipb_member_id=fixture; ipb_pass_hash=fixture";

    struct NoInvalidation;

    impl AdminCredentialRuntimeWriter for NoInvalidation {
        fn mark_invalid(&self, _credential_id: Uuid, _now: Timestamp) -> ApplicationFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    struct ValidOwner;

    impl AdminCredentialRuntimeOwnership for ValidOwner {
        fn check(&self) -> ApplicationFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn is_valid(&self) -> bool {
            true
        }

        fn mail_runtime_binding(&self) -> Option<MailRuntimeBinding> {
            Some(MailRuntimeBinding { generation: 1 })
        }

        fn revoke_internal_use(&self) {}
    }

    fn binding() -> MailAccountEvidenceBinding {
        let mut binding = MailAccountEvidenceBinding {
            credential_id: Uuid::from_u128(1),
            credential_version: 1,
            runtime: MailRuntimeBinding { generation: 1 },
            authenticated_uid: "1000000".to_owned(),
            player_name: "FixtureBot".to_owned(),
            world: SettlementWorld::Main,
            observed_at: Timestamp::now(),
            protocol_version: "hv_uid_forum_profile_armory_v1",
            digest: [0; 32],
        };
        binding.digest = binding.compute_digest();
        binding
    }

    fn facts(tradeable: bool) -> EquipmentFacts {
        EquipmentFacts {
            external_equipment_id: "00042".to_owned(),
            name: "Fixture Blade".to_owned(),
            category: "Weapon".to_owned(),
            base_name: "Sword".to_owned(),
            quality: "Superior".to_owned(),
            prefix: "Sharp".to_owned(),
            suffix: "the Fox".to_owned(),
            level: EquipmentLevel::Assigned(12),
            tradeable,
            stats: BTreeMap::new(),
        }
    }

    #[test]
    fn authenticated_equipment_observation_preserves_tradeability() {
        let observation = custody_observation_from_facts(&binding(), facts(true), Timestamp::now())
            .expect("可交易观察");
        assert_eq!(observation.world, SettlementWorld::Main);
        assert_eq!(observation.external_equipment_id, "42");
        assert!(observation.tradeable);
        assert_eq!(observation.level, Some(12));
        assert_ne!(observation.digest, [0; 32]);
    }

    #[test]
    fn soulbound_negative_is_kept_and_never_upgraded() {
        let observation =
            custody_observation_from_facts(&binding(), facts(false), Timestamp::now())
                .expect("魂绑仍是观察");
        assert!(!observation.tradeable);
    }

    #[tokio::test]
    async fn custody_read_rejects_mismatched_session_uid_before_equipment_get() {
        let server = MailDouble::start().await.expect("loopback");
        let client = MoogleMailClient::with_config(MoogleMailClientConfig {
            endpoint: server.endpoint(),
            maximum_response_bytes: 8 * 1024 * 1024,
            minimum_request_interval: Duration::from_millis(1),
            request_timeout: Duration::from_secs(2),
        })
        .expect("loopback client");
        let credentials = MoogleMailCredentialSet::from_runtime_credentials(
            vec![MoogleMailRuntimeCredential {
                credential_id: Uuid::from_u128(1),
                state_version: 1,
                world: Some(MoogleMailWorld::Main),
                player_name: Some("FixtureBot".to_owned()),
                cookie: MoogleMailCookie::new(COOKIE).expect("fixture cookie"),
            }],
            Arc::new(NoInvalidation),
            Some(Arc::new(ValidOwner)),
        )
        .expect("persistent fixture credentials");
        let adapter = EquipmentSourceAdapter {
            client: build_equipment_http_client().expect("equipment client"),
            endpoint: server.endpoint(),
            policy: EquipmentSourcePolicy {
                allowlist_enabled: false,
                minimum_request_interval: Duration::from_millis(1),
                request_timeout: Duration::from_secs(2),
                ..EquipmentSourcePolicy::default()
            },
            credentials: None,
            mail_client: None,
            diagnostics: None,
            diagnostic_reporter: None,
            next_request_at: Mutex::new(tokio::time::Instant::now()),
        }
        .with_mooglemail_credentials(credentials, client);
        let mut mismatched = binding();
        mismatched.authenticated_uid = "9999999".to_owned();
        mismatched.digest = mismatched.compute_digest();
        let attachment = MailEquipmentAttachment {
            external_equipment_id: "900001".to_owned(),
            key: EquipmentKey::new("0123456789").expect("fixture key"),
        };
        let result = adapter
            .read_custody_equipment(
                &mismatched,
                attachment,
                Instant::now() + Duration::from_secs(3),
            )
            .await;
        assert_eq!(result, Err(CustodyEquipmentError::IdentityUnavailable));
        let requests = server.requests();
        assert!(requests.iter().any(|request| request.contains("s=Character")));
        assert!(!requests.iter().any(|request| request.contains("/equip/")));
    }
}
