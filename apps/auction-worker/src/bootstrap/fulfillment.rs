//! MoogleMail 结算履约 evidence reader 与不可用降级端口。

use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use auction_application::settlement::{
    InternalEvidenceError, InternalEvidenceFuture, InternalEvidenceRequest,
    PreparedSettlementInternalEvidence, SettlementInternalEvidenceReader,
};
use auction_application::settlement::{
    SettlementFulfillmentBalanceEvidence, SettlementFulfillmentCustodyReader,
    SettlementFulfillmentEvidenceReader, SettlementFulfillmentSentEvidence,
};
use auction_domain::SettlementWorld;
use auction_integrations::mooglemail::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailGateway, MoogleMailInventoryCategory,
    MoogleMailWorld,
};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;

pub(super) struct MoogleMailFulfillmentCustodyReader {
    pub(super) gateway: Arc<MoogleMailGateway>,
}

impl SettlementFulfillmentCustodyReader for MoogleMailFulfillmentCustodyReader {
    fn read_custody(
        &self,
        request: InternalEvidenceRequest,
    ) -> InternalEvidenceFuture<'_, PreparedSettlementInternalEvidence> {
        // 装备详情可公开读取，不能证明托管。仅复用当前账号、世界与独占会话的 Armory 正向证据。
        self.gateway.prepare_internal_evidence(request)
    }
}

pub(super) struct MoogleMailFulfillmentEvidenceReader {
    pub(super) gateways: HashMap<MoogleMailWorld, Arc<MoogleMailGateway>>,
}

fn fulfillment_gateway(
    gateways: &HashMap<MoogleMailWorld, Arc<MoogleMailGateway>>,
    world: SettlementWorld,
    unavailable_detail: &'static str,
) -> Result<(MoogleMailWorld, Arc<MoogleMailGateway>), ApplicationError> {
    let world = MoogleMailWorld::from(world);
    let gateway = gateways
        .get(&world)
        .cloned()
        .ok_or(ApplicationError::CapabilityUnavailable(unavailable_detail))?;
    Ok((world, gateway))
}

/// 复用既有 gateway/credential set/ownership，不构造第二 Cookie 客户端。
/// 此兼容端口只返回身份；内部 satisfy 由同一 gateway 的独立组合证据端口授权。
impl auction_application::settlement::SettlementFulfillmentBotIdentityReader
    for MoogleMailFulfillmentEvidenceReader
{
    fn read_verified_player_identity(
        &self,
        world: SettlementWorld,
    ) -> ApplicationFuture<'_, Option<auction_application::admin::VerifiedMailPlayerIdentity>> {
        Box::pin(async move {
            let (world, gateway) = fulfillment_gateway(
                &self.gateways,
                world,
                "MoogleMail player identity world unavailable",
            )?;
            gateway.read_verified_player_identity(world).await.map_err(mooglemail_evidence_error)
        })
    }
}

impl SettlementFulfillmentEvidenceReader for MoogleMailFulfillmentEvidenceReader {
    fn read_sent(
        &self,
        world: SettlementWorld,
        external_mail_id: String,
    ) -> ApplicationFuture<'_, SettlementFulfillmentSentEvidence> {
        let (world, gateway) = match fulfillment_gateway(
            &self.gateways,
            world,
            "MoogleMail Sent evidence world unavailable",
        ) {
            Ok(value) => value,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        Box::pin(async move {
            let snapshot = gateway
                .read_sent(world, &external_mail_id, Utc::now())
                .await
                .map_err(mooglemail_evidence_error)?;
            Ok(SettlementFulfillmentSentEvidence {
                external_mail_id: snapshot.external_mail_id,
                world: snapshot.world.into(),
                recipient: snapshot.recipient,
                subject: snapshot.subject,
                remaining_credits: snapshot.remaining_credits,
                cod_pending: snapshot.cod_pending,
                remaining_equipment_ids: snapshot.remaining_equipment_ids,
                collected: snapshot.collected,
                observed_at: snapshot.observed_at,
            })
        })
    }

    fn read_balance(
        &self,
        world: SettlementWorld,
        observed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, SettlementFulfillmentBalanceEvidence> {
        let (world, gateway) = match fulfillment_gateway(
            &self.gateways,
            world,
            "MoogleMail Balance evidence world unavailable",
        ) {
            Ok(value) => value,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        Box::pin(async move {
            let snapshot = gateway
                .read_balance(world, observed_at)
                .await
                .map_err(mooglemail_evidence_error)?;
            Ok(SettlementFulfillmentBalanceEvidence {
                world: snapshot.world.into(),
                credits: snapshot.credits,
                observed_at: snapshot.observed_at,
            })
        })
    }
}

pub(super) struct UnavailableFulfillmentEvidenceReader;

pub(super) struct UnavailableFulfillmentCustodyReader;

impl SettlementFulfillmentCustodyReader for UnavailableFulfillmentCustodyReader {
    fn read_custody(
        &self,
        _request: InternalEvidenceRequest,
    ) -> InternalEvidenceFuture<'_, PreparedSettlementInternalEvidence> {
        Box::pin(async { Err(InternalEvidenceError::Unavailable) })
    }
}

impl SettlementFulfillmentEvidenceReader for UnavailableFulfillmentEvidenceReader {
    fn read_sent(
        &self,
        _world: SettlementWorld,
        _external_mail_id: String,
    ) -> ApplicationFuture<'_, SettlementFulfillmentSentEvidence> {
        Box::pin(async {
            Err(ApplicationError::CapabilityUnavailable(
                "MoogleMail Sent evidence reader unavailable",
            ))
        })
    }

    fn read_balance(
        &self,
        _world: SettlementWorld,
        _observed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, SettlementFulfillmentBalanceEvidence> {
        Box::pin(async {
            Err(ApplicationError::CapabilityUnavailable(
                "MoogleMail Balance evidence reader unavailable",
            ))
        })
    }
}

fn mooglemail_evidence_error(error: MoogleMailError) -> ApplicationError {
    match error.category {
        MoogleMailErrorCategory::NotObserved => {
            return ApplicationError::FulfillmentEvidenceNotObserved;
        }
        MoogleMailErrorCategory::AccountBusy => {
            return ApplicationError::FulfillmentAccountBusy;
        }
        _ => {}
    }
    // 其余远端失败保留安全分类：协议漂移、会话失效等不再显示为本地仓储故障。
    ApplicationError::RemoteDependency {
        kind: error.safe_kind(),
        detail: format!("MoogleMail {} 证据读取失败", error.category),
    }
}

/// 只读 Armory 观察：复用既有 gateway/凭据集合，不构造第二 Cookie 客户端，
/// 也不执行任何 organize/解锁/寄送写操作。
pub(super) struct MoogleMailAssetInventoryReader {
    pub(super) gateways: HashMap<MoogleMailWorld, Arc<MoogleMailGateway>>,
}

impl auction_application::reconciliation::AssetInventoryReader for MoogleMailAssetInventoryReader {
    fn read_inventory(
        &self,
        world: SettlementWorld,
        observed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, auction_application::reconciliation::AssetArmoryObservation> {
        Box::pin(async move {
            let moogle_world = MoogleMailWorld::from(world);
            let gateway = self.gateways.get(&moogle_world).cloned().ok_or(
                ApplicationError::CapabilityUnavailable("MoogleMail inventory world unavailable"),
            )?;
            let mut equipment_ids = Vec::new();
            let mut complete = true;
            let mut failure_code = None;
            for category in MoogleMailInventoryCategory::ALL {
                match gateway.read_inventory_ids(moogle_world, category, observed_at).await {
                    Ok(snapshot) => equipment_ids.extend(snapshot.equipment_ids),
                    Err(error) => {
                        complete = false;
                        failure_code = Some(inventory_failure_code(&error));
                        break;
                    }
                }
            }
            equipment_ids.sort_unstable();
            equipment_ids.dedup();
            Ok(auction_application::reconciliation::AssetArmoryObservation {
                world,
                observed_at,
                equipment_ids,
                complete,
                failure_code,
            })
        })
    }
}

fn inventory_failure_code(error: &MoogleMailError) -> &'static str {
    match error.category {
        MoogleMailErrorCategory::AccountBusy => "account_busy",
        MoogleMailErrorCategory::SessionExpired => "session_expired",
        MoogleMailErrorCategory::CredentialUnavailable => "credential_unavailable",
        MoogleMailErrorCategory::NotObserved => "not_observed",
        MoogleMailErrorCategory::ProtocolDrift => "protocol_drift",
        _ => "inventory_unavailable",
    }
}
