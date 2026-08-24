//! MoogleMail 结算履约 evidence reader 与不可用降级端口。

#[cfg(any(feature = "postgres", feature = "sqlite"))]
use super::*;

#[cfg(feature = "postgres")]
pub(super) struct MoogleMailFulfillmentEvidenceReader {
    pub(super) gateways: HashMap<MoogleMailWorld, Arc<MoogleMailGateway>>,
}

#[cfg(feature = "postgres")]
fn fulfillment_gateway(
    gateways: &HashMap<MoogleMailWorld, Arc<MoogleMailGateway>>,
    world: SettlementWorld,
    unavailable_detail: &'static str,
) -> Result<(MoogleMailWorld, Arc<MoogleMailGateway>), ApplicationError> {
    let world = mooglemail_world(world)?;
    let gateway = gateways
        .get(&world)
        .cloned()
        .ok_or(ApplicationError::CapabilityUnavailable(unavailable_detail))?;
    Ok((world, gateway))
}

#[cfg(feature = "postgres")]
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
                world: settlement_world(snapshot.world),
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
            let snapshot =
                gateway.read_balance(world, Utc::now()).await.map_err(mooglemail_evidence_error)?;
            Ok(SettlementFulfillmentBalanceEvidence {
                world: settlement_world(snapshot.world),
                credits: snapshot.credits,
                observed_at: snapshot.observed_at,
            })
        })
    }
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub(super) struct UnavailableFulfillmentEvidenceReader;

#[cfg(any(feature = "postgres", feature = "sqlite"))]
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
    ) -> ApplicationFuture<'_, SettlementFulfillmentBalanceEvidence> {
        Box::pin(async {
            Err(ApplicationError::CapabilityUnavailable(
                "MoogleMail Balance evidence reader unavailable",
            ))
        })
    }
}

#[cfg(feature = "postgres")]
fn mooglemail_world(world: SettlementWorld) -> Result<MoogleMailWorld, ApplicationError> {
    Ok(match world {
        SettlementWorld::Main => MoogleMailWorld::Main,
        SettlementWorld::Isekai => MoogleMailWorld::Isekai,
    })
}

#[cfg(feature = "postgres")]
fn settlement_world(world: MoogleMailWorld) -> SettlementWorld {
    match world {
        MoogleMailWorld::Main => SettlementWorld::Main,
        MoogleMailWorld::Isekai => SettlementWorld::Isekai,
    }
}

#[cfg(feature = "postgres")]
fn mooglemail_evidence_error(error: MoogleMailError) -> ApplicationError {
    if error.category == MoogleMailErrorCategory::NotObserved {
        return ApplicationError::FulfillmentEvidenceNotObserved;
    }
    auction_application::RepositoryError::Failed(format!(
        "MoogleMail {} 证据读取失败",
        error.category
    ))
    .into()
}
