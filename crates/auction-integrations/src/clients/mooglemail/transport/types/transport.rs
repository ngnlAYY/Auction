//! MoogleMail delivery transport 的运行时装配状态。

use super::templates::MoogleMailNotificationTemplates;
use crate::EquipmentKeyOpener;
use crate::clients::mooglemail::{
    MoogleMailClient, MoogleMailCredentialSet, MoogleMailParser, MoogleMailSender,
};
use auction_application::mail::MailLoginKeyReader;
use auction_application::settlement::SettlementFulfillmentEquipmentSecretReader;
use std::sync::Arc;

/// MoogleMail delivery 的受控远端 transport。
pub struct MoogleMailDeliveryTransport {
    pub(crate) client: MoogleMailClient,
    pub(crate) credentials: MoogleMailCredentialSet,
    pub(crate) sender: MoogleMailSender,
    pub(crate) parser: MoogleMailParser,
    pub(crate) login_keys: Arc<dyn MailLoginKeyReader>,
    pub(crate) settlement_equipment_secrets:
        Option<Arc<dyn SettlementFulfillmentEquipmentSecretReader>>,
    pub(crate) settlement_equipment_opener: Option<EquipmentKeyOpener>,
    pub(crate) templates: MoogleMailNotificationTemplates,
    pub(crate) cash_evidence_gateway: Option<Arc<super::super::super::MoogleMailGateway>>,
    pub(crate) funding_policy_reader:
        Option<Arc<dyn auction_application::settlement::FundingPolicyReader>>,
}
