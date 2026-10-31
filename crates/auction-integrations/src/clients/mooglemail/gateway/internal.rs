//! 同 Cookie/operation guard 的联合证据；没有资金授权、业务状态迁移或远端写入。

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use auction_application::{
    ForumTargetProfileReader, INTERNAL_EVIDENCE_PROTOCOL, InternalEvidenceError as Error,
    InternalEvidenceFuture, InternalEvidenceLease, InternalEvidenceRequest,
    MailAccountEvidenceBinding, PreparedSettlementInternalEvidence,
    SettlementInternalEvidenceReader, SettlementInternalEvidenceSnapshot,
    VerifiedMailEquipmentPresence, VerifiedMainCashObservation,
};
use auction_domain::SettlementWorld;
use chrono::Utc;

use super::super::protocol::MoogleMailInventoryCategory;
use super::*;

mod diagnostics;
use diagnostics::{EvidenceStage as Stage, at};

impl MoogleMailGateway {
    /// 注入既有论坛只读能力；不启动任何发帖/PM任务，也不向论坛传递 MM Cookie。
    pub fn with_forum_profile_reader(mut self, reader: Arc<dyn ForumTargetProfileReader>) -> Self {
        self.profile_reader = Some(reader);
        self
    }

    /// 兼容旧身份端口。预期名称 None 也读取真实 UID/目标资料，失败不返回 None。
    pub async fn read_verified_player_identity(
        &self,
        world: MoogleMailWorld,
    ) -> Result<Option<auction_application::VerifiedMailPlayerIdentity>, MoogleMailError> {
        let deadline = Instant::now() + self.config.fetch_budget.min(Duration::from_secs(30));
        let result = tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), async {
            let session = self
                .credentials
                .begin_exclusive(world, MoogleMailOperation::Identity)
                .await
                .map_err(map_read_error)?;
            let identity = self.read_identity_in_session(&session, world, deadline).await?;
            session.check_internal_current(deadline).await?;
            Ok::<_, Error>(identity)
        })
        .await;
        let identity =
            result.map_err(|_| identity_error(world))?.map_err(|_| identity_error(world))?;
        Ok(Some(auction_application::VerifiedMailPlayerIdentity {
            credential_id: identity.credential_id,
            credential_version: identity.credential_version,
            world: identity.world,
            player_name: identity.player_name,
            observed_at: identity.observed_at,
            proof_source: auction_application::MailPlayerIdentityProofSource::AuthenticatedHvUidForumTargetProfile,
            proof_digest: identity.digest,
        }))
    }

    /// 给现金发送准备复用的同 session 入口；不得嵌套 begin_exclusive 或复制客户端。
    /// 调用方保留 session 到 sending 事务结束，并对取消/未知结果撤销相同 owner。
    pub(crate) async fn read_internal_evidence_in_session(
        &self,
        session: &MoogleMailCredentialSession,
        request: &InternalEvidenceRequest,
    ) -> Result<SettlementInternalEvidenceSnapshot, Error> {
        if !request.validate() {
            return Err(at(
                Stage::Request,
                MoogleMailWorld::from(request.world),
                Error::InvalidRequest,
            ));
        }
        if !self.credentials.owns_session(session) {
            return Err(at(
                Stage::SessionOwnership,
                MoogleMailWorld::from(request.world),
                Error::Unsupported,
            ));
        }
        let deadline = request
            .deadline
            .min(Instant::now() + self.config.fetch_budget.min(Duration::from_secs(30)));
        // 提交预留一秒，不为每次 GET 重置期限；外层预算仍包含 mutex 排队。
        let read_deadline = deadline.checked_sub(Duration::from_secs(1)).ok_or(Error::Expired)?;
        if read_deadline <= Instant::now() {
            return Err(Error::Expired);
        }
        tokio::time::timeout_at(tokio::time::Instant::from_std(read_deadline), async {
            let observed_from = Utc::now();
            let remaining = deadline.saturating_duration_since(Instant::now());
            let expires_at = observed_from
                + chrono::Duration::from_std(remaining).map_err(|_| Error::Expired)?;
            let world = MoogleMailWorld::from(request.world);
            session
                .check_internal_current(read_deadline)
                .await
                .map_err(|error| at(Stage::OwnerCheck, world, error))?;
            let identity = self.read_identity_in_session(session, world, read_deadline).await?;
            self.require_clean_compose(session, world).await?;
            let mut equipment = None;
            if let Some(target) = &request.equipment {
                let equipment_world = MoogleMailWorld::from(target.world);
                let binding = if equipment_world == world {
                    identity.clone()
                } else {
                    let binding = self
                        .read_identity_in_session(session, equipment_world, read_deadline)
                        .await?;
                    if !identity.same_account(&binding) {
                        return Err(at(Stage::AccountBinding, equipment_world, Error::Unproven));
                    }
                    self.require_clean_compose(session, equipment_world).await?;
                    binding
                };
                let mut category_found = None;
                for category in MoogleMailInventoryCategory::ALL {
                    let target_url = self.client.inventory(equipment_world, category);
                    let document = self
                        .read_session_document(session, &target_url)
                        .await
                        .map_err(|error| at(Stage::InventoryGet, equipment_world, error))?;
                    if self
                        .parser
                        .parse_inventory_presence(
                            &document,
                            equipment_world,
                            category,
                            &target.external_equipment_id,
                        )
                        .map_err(|error| {
                            at(Stage::InventoryParse, equipment_world, map_read_error(error))
                        })?
                    {
                        category_found = Some(category);
                        break;
                    }
                }
                let category = category_found
                    .ok_or_else(|| at(Stage::InventoryMissing, equipment_world, Error::Unproven))?;
                let mut presence = VerifiedMailEquipmentPresence {
                    binding,
                    equipment_id: target.equipment_id,
                    external_equipment_id: target.external_equipment_id.clone(),
                    category: category.as_str().to_owned(),
                    observed_at: Utc::now(),
                    digest: [0; 32],
                };
                presence.digest = presence.compute_digest();
                equipment = Some(presence);
            }
            let mut main_cash = None;
            if request.observe_main_cash {
                let binding = if world == MoogleMailWorld::Main {
                    identity.clone()
                } else if let Some(presence) = &equipment {
                    if presence.binding.world == SettlementWorld::Main {
                        presence.binding.clone()
                    } else {
                        self.read_identity_in_session(session, MoogleMailWorld::Main, read_deadline)
                            .await?
                    }
                } else {
                    self.read_identity_in_session(session, MoogleMailWorld::Main, read_deadline)
                        .await?
                };
                if !identity.same_account(&binding) {
                    return Err(at(Stage::AccountBinding, MoogleMailWorld::Main, Error::Unproven));
                }
                // 同 guard 内额外读取不重新取得非重入锁。
                self.require_clean_compose(session, MoogleMailWorld::Main).await?;
                let target = self.client.balance(MoogleMailWorld::Main).map_err(|error| {
                    at(Stage::CashGet, MoogleMailWorld::Main, map_read_error(error))
                })?;
                let document = self
                    .read_session_document(session, &target)
                    .await
                    .map_err(|error| at(Stage::CashGet, MoogleMailWorld::Main, error))?;
                let document = self
                    .parser
                    .evidence_document(
                        &document,
                        MoogleMailWorld::Main,
                        MoogleMailOperation::Balance,
                    )
                    .map_err(|error| {
                        at(Stage::CashWorld, MoogleMailWorld::Main, map_read_error(error))
                    })?;
                let observed_at = Utc::now();
                let balance = self
                    .parser
                    .parse_balance_document(&document, MoogleMailWorld::Main, observed_at)
                    .map_err(|error| {
                        at(Stage::CashParse, MoogleMailWorld::Main, map_read_error(error))
                    })?;
                let mut observation = VerifiedMainCashObservation {
                    binding,
                    credits: balance.credits,
                    observed_at,
                    digest: [0; 32],
                };
                observation.digest = observation.compute_digest();
                main_cash = Some(observation);
            }
            session
                .check_internal_current(read_deadline)
                .await
                .map_err(|error| at(Stage::OwnerCheck, world, error))?;
            let mut snapshot = SettlementInternalEvidenceSnapshot {
                operation_id: request.operation_id,
                purpose: request.purpose.clone(),
                identity,
                equipment,
                main_cash,
                sent_collection: None,
                observed_from,
                expires_at,
                digest: [0; 32],
            };
            snapshot.digest = snapshot.compute_digest();
            if !snapshot.valid_at(Utc::now()) {
                return Err(at(Stage::SnapshotProof, world, Error::Expired));
            }
            Ok(snapshot)
        })
        .await
        .map_err(|_| at(Stage::Deadline, MoogleMailWorld::from(request.world), Error::Expired))?
    }

    pub(crate) async fn read_identity_in_session(
        &self,
        session: &MoogleMailCredentialSession,
        world: MoogleMailWorld,
        deadline: Instant,
    ) -> Result<MailAccountEvidenceBinding, Error> {
        if !self.credentials.owns_session(session) {
            return Err(at(Stage::SessionOwnership, world, Error::Unsupported));
        }
        let (credential_id, credential_version, runtime, expected) = session
            .internal_credential_binding(world)
            .map_err(|error| at(Stage::RuntimeBinding, world, error))?;
        let reader = self
            .profile_reader
            .as_ref()
            .ok_or_else(|| at(Stage::ProfileCapability, world, Error::Unsupported))?;
        let observed_from = Utc::now();
        let target = self.client.identity(world);
        let document = self
            .read_session_document(session, &target)
            .await
            .map_err(|error| at(Stage::IdentityGet, world, error))?;
        let uid = self
            .parser
            .parse_authenticated_uid(&document, world)
            .map_err(|error| at(Stage::UidParse, world, map_read_error(error)))?;
        let profile = reader
            .read_target_profile(uid.clone(), deadline)
            .await
            .map_err(|error| at(Stage::ProfileRead, world, error))?;
        if profile.target_uid != uid
            || !auction_application::valid_mail_player_name(&profile.player_name)
            || expected.is_some_and(|value| value != profile.player_name)
            || profile.protocol_version != "forum_target_profile_mid_v1"
            || profile.digest == [0; 32]
            || profile.observed_at < observed_from
            || profile.observed_at > Utc::now()
        {
            return Err(at(Stage::ProfileValidation, world, Error::Unproven));
        }
        let mut binding = MailAccountEvidenceBinding {
            credential_id,
            credential_version,
            runtime,
            authenticated_uid: uid,
            player_name: profile.player_name,
            world: world.into(),
            observed_at: Utc::now(),
            protocol_version: INTERNAL_EVIDENCE_PROTOCOL,
            digest: [0; 32],
        };
        binding.digest = binding.compute_digest();
        Ok(binding)
    }

    pub(super) async fn require_clean_compose(
        &self,
        session: &MoogleMailCredentialSession,
        world: MoogleMailWorld,
    ) -> Result<(), Error> {
        session
            .internal_credential_binding(world)
            .map_err(|error| at(Stage::RuntimeBinding, world, error))?;
        let target = self
            .client
            .compose(world)
            .map_err(|error| at(Stage::ComposeGet, world, map_read_error(error)))?;
        let document = self
            .read_session_document(session, &target)
            .await
            .map_err(|error| at(Stage::ComposeGet, world, error))?;
        let document = self
            .parser
            .evidence_document(&document, world, MoogleMailOperation::Compose)
            .map_err(|error| at(Stage::ComposeWorld, world, map_read_error(error)))?;
        if !self
            .parser
            .parse_compose_document(&document, world)
            .map_err(|error| at(Stage::ComposeParse, world, map_read_error(error)))?
            .is_clean()
        {
            return Err(at(Stage::ComposeDirty, world, Error::Unproven));
        }
        Ok(())
    }

    async fn read_session_document(
        &self,
        session: &MoogleMailCredentialSession,
        target: &MoogleMailTarget,
    ) -> Result<Vec<u8>, Error> {
        session.internal_credential_binding(target.world())?;
        match self.client.get(target, session.cookie()).await {
            Ok(document) => Ok(document),
            Err(error) => {
                // 仅 MM 明确登录失效才回写该凭据；论坛读取失败不走此分支。
                // 即使回写失败，sticky 撤销仍已生效，不轮换 Cookie 重建本次证明。
                if error.category == MoogleMailErrorCategory::SessionExpired {
                    let _ = session.invalidate_internal_current().await;
                }
                Err(map_read_error(error))
            }
        }
    }
}

impl SettlementInternalEvidenceReader for MoogleMailGateway {
    fn prepare_internal_evidence_with_sent(
        &self,
        request: InternalEvidenceRequest,
        target: auction_application::InternalSentCollectionTarget,
    ) -> InternalEvidenceFuture<'_, PreparedSettlementInternalEvidence> {
        Box::pin(self.prepare_with_sent(request, target))
    }

    fn prepare_internal_evidence(
        &self,
        mut request: InternalEvidenceRequest,
    ) -> InternalEvidenceFuture<'_, PreparedSettlementInternalEvidence> {
        Box::pin(async move {
            if !request.validate() {
                return Err(at(
                    Stage::Request,
                    MoogleMailWorld::from(request.world),
                    Error::InvalidRequest,
                ));
            }
            request.deadline = auction_application::internal_evidence_deadline(request.deadline)
                .min(Instant::now() + self.config.fetch_budget.min(Duration::from_secs(30)));
            let deadline = request.deadline;
            tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), async {
                let session = self
                    .credentials
                    .begin_exclusive(
                        MoogleMailWorld::from(request.world),
                        MoogleMailOperation::Identity,
                    )
                    .await
                    .map_err(|error| {
                        at(
                            Stage::SessionAcquire,
                            MoogleMailWorld::from(request.world),
                            map_read_error(error),
                        )
                    })?;
                let snapshot = self.read_internal_evidence_in_session(&session, &request).await?;
                PreparedSettlementInternalEvidence::new(snapshot, Box::new(session), deadline)
                    .map_err(|error| {
                        at(Stage::SnapshotGuard, MoogleMailWorld::from(request.world), error)
                    })
            })
            .await
            .map_err(|_| {
                at(Stage::Deadline, MoogleMailWorld::from(request.world), Error::Expired)
            })?
        })
    }
}

impl InternalEvidenceLease for MoogleMailCredentialSession {
    fn is_current(&self) -> bool {
        self.internal_credential_binding(self.internal_world()).is_ok()
    }

    fn check_current(&self, deadline: Instant) -> InternalEvidenceFuture<'_, ()> {
        Box::pin(self.check_internal_current(deadline))
    }

    fn revoke(&self) {
        self.revoke_internal_owner();
    }
}

pub(super) fn map_read_error(error: MoogleMailError) -> Error {
    diagnostics::read_error(error.operation, error.world, error.category);
    match error.category {
        MoogleMailErrorCategory::CredentialUnavailable
        | MoogleMailErrorCategory::RemoteTemporary
        | MoogleMailErrorCategory::RateLimited
        | MoogleMailErrorCategory::AccountBusy => Error::Unavailable,
        _ => Error::Unproven,
    }
}

fn identity_error(world: MoogleMailWorld) -> MoogleMailError {
    operation_error(MoogleMailOperation::Identity, world, MoogleMailErrorCategory::IdentityUnproven)
}
