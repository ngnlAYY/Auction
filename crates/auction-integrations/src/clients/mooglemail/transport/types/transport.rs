//! MoogleMail delivery transport 的运行时装配状态。

use super::super::*;
use super::templates::MoogleMailNotificationTemplates;

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
}
