//! MoogleMail 结算履约 evidence reader 与不可用降级端口。

#[cfg(any(feature = "postgres", feature = "sqlite"))]
use super::*;
#[cfg(feature = "postgres")]
use serde::Serialize;
#[cfg(feature = "postgres")]
use sha2::{Digest, Sha256};

#[cfg(feature = "postgres")]
pub(super) struct EquipmentFulfillmentCustodyReader {
    secrets: Arc<dyn SettlementFulfillmentEquipmentSecretReader>,
    opener: EquipmentKeyOpener,
    equipment: Arc<dyn MailEquipmentReader>,
    clock: Arc<dyn Clock>,
}

#[cfg(feature = "postgres")]
impl EquipmentFulfillmentCustodyReader {
    pub(super) fn new(
        secrets: Arc<dyn SettlementFulfillmentEquipmentSecretReader>,
        opener: EquipmentKeyOpener,
        equipment: Arc<dyn MailEquipmentReader>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { secrets, opener, equipment, clock }
    }
}

#[cfg(feature = "postgres")]
impl SettlementFulfillmentCustodyReader for EquipmentFulfillmentCustodyReader {
    fn read_custody(
        &self,
        equipment_id: auction_domain::EquipmentId,
        world: SettlementWorld,
        external_equipment_id: String,
    ) -> ApplicationFuture<'_, SettlementFulfillmentCustodySnapshot> {
        Box::pin(async move {
            if equipment_id.is_nil()
                || external_equipment_id.is_empty()
                || external_equipment_id != external_equipment_id.trim()
            {
                return Err(ApplicationError::FulfillmentDependencyUnavailable);
            }
            let secret = self
                .secrets
                .read_equipment_secret(equipment_id)
                .await
                .map_err(|_| ApplicationError::FulfillmentDependencyUnavailable)?;
            if secret.equipment_id != equipment_id
                || secret.external_equipment_id != external_equipment_id
            {
                return Err(ApplicationError::FulfillmentDependencyMismatch);
            }
            let key = self
                .opener
                .open(equipment_id, &secret.envelope)
                .map_err(|_| ApplicationError::FulfillmentDependencyUnavailable)?;
            let mut source = self
                .equipment
                .read(
                    settlement_world_id(world).to_owned(),
                    MailEquipmentAttachment {
                        external_equipment_id: external_equipment_id.clone(),
                        key,
                    },
                )
                .await
                .map_err(custody_source_error)?;
            source.clear_secret();
            if source.external_equipment_id != external_equipment_id {
                return Err(ApplicationError::FulfillmentDependencyMismatch);
            }
            if !source.attributes.is_object() {
                return Err(ApplicationError::FulfillmentDependencyUnavailable);
            }
            let observed_at = self.clock.now();
            let digest =
                custody_digest(equipment_id, world, &external_equipment_id, &source.attributes)?;
            Ok(SettlementFulfillmentCustodySnapshot {
                equipment_id,
                world,
                external_equipment_id,
                custodied: true,
                observed_at,
                digest,
            })
        })
    }
}

#[cfg(feature = "postgres")]
#[derive(Serialize)]
struct CustodyDigestPayload<'a> {
    equipment_id: String,
    world: &'static str,
    external_equipment_id: &'a str,
    attributes: &'a serde_json::Value,
}

#[cfg(feature = "postgres")]
fn custody_digest(
    equipment_id: auction_domain::EquipmentId,
    world: SettlementWorld,
    external_equipment_id: &str,
    attributes: &serde_json::Value,
) -> Result<[u8; 32], ApplicationError> {
    let encoded = serde_json::to_vec(&CustodyDigestPayload {
        equipment_id: equipment_id.to_string(),
        world: settlement_world_id(world),
        external_equipment_id,
        attributes,
    })
    .map_err(|_| ApplicationError::FulfillmentDependencyUnavailable)?;
    let digest = Sha256::digest(go_json_compatible_bytes(encoded));
    Ok(digest.into())
}

#[cfg(feature = "postgres")]
fn go_json_compatible_bytes(encoded: Vec<u8>) -> Vec<u8> {
    let mut output = Vec::with_capacity(encoded.len());
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
            output.extend_from_slice(replacement);
            index += consumed;
        } else {
            output.push(encoded[index]);
            index += 1;
        }
    }
    output
}

#[cfg(feature = "postgres")]
fn custody_source_error(error: ApplicationError) -> ApplicationError {
    match error {
        ApplicationError::FulfillmentDependencyMismatch => error,
        _ => ApplicationError::FulfillmentDependencyUnavailable,
    }
}

#[cfg(feature = "postgres")]
fn settlement_world_id(world: SettlementWorld) -> &'static str {
    match world {
        SettlementWorld::Main => "main",
        SettlementWorld::Isekai => "isekai",
    }
}

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
pub(super) struct UnavailableFulfillmentCustodyReader;

#[cfg(any(feature = "postgres", feature = "sqlite"))]
impl SettlementFulfillmentCustodyReader for UnavailableFulfillmentCustodyReader {
    fn read_custody(
        &self,
        _equipment_id: auction_domain::EquipmentId,
        _world: SettlementWorld,
        _external_equipment_id: String,
    ) -> ApplicationFuture<'_, SettlementFulfillmentCustodySnapshot> {
        Box::pin(async { Err(ApplicationError::FulfillmentDependencyUnavailable) })
    }
}

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
        _observed_at: DateTime<Utc>,
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
    match error.category {
        MoogleMailErrorCategory::NotObserved => {
            return ApplicationError::FulfillmentEvidenceNotObserved;
        }
        MoogleMailErrorCategory::AccountBusy => {
            return ApplicationError::FulfillmentAccountBusy;
        }
        _ => {}
    }
    auction_application::RepositoryError::Failed(format!(
        "MoogleMail {} 证据读取失败",
        error.category
    ))
    .into()
}
