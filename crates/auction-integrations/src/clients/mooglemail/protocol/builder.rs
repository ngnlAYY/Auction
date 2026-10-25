//! MoogleMail 固定 endpoint 的安全 URL 构造器。

use url::Url;

use super::super::{MoogleMailError, MoogleMailErrorCategory};
use super::model::{
    MoogleMailEquipmentKey, MoogleMailOperation, MoogleMailTarget, MoogleMailWorld,
};
use super::validation::{endpoint_origin, valid_decimal_id, valid_endpoint, valid_opaque_id};

/// 只构造批准过的 MoogleMail URL，不接受调用方传入 raw path/query。
#[derive(Clone)]
pub(crate) struct MoogleMailUrlBuilder {
    pub(crate) endpoint: Url,
    pub(crate) origin: String,
}

impl MoogleMailUrlBuilder {
    pub(crate) fn new(endpoint: Url) -> Result<Self, MoogleMailError> {
        if !valid_endpoint(&endpoint) {
            return Err(MoogleMailError::new(
                MoogleMailOperation::Inbox,
                MoogleMailWorld::Main,
                MoogleMailErrorCategory::Configuration,
                false,
            ));
        }
        let origin = endpoint_origin(&endpoint);
        Ok(Self { endpoint, origin })
    }

    pub(crate) fn inbox_page(
        &self,
        world: MoogleMailWorld,
        page: usize,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        let mut target = self.moogle_mail(world, MoogleMailOperation::Inbox, "inbox", None)?;
        if page > 0 {
            target.url.query_pairs_mut().append_pair("page", &page.to_string());
        }
        Ok(target)
    }

    /// Read Mail 使用远端真实页签值，不映射为 Inbox 或详情请求。
    pub(crate) fn read_mail_page(
        &self,
        world: MoogleMailWorld,
        page: usize,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        let mut target =
            self.moogle_mail(world, MoogleMailOperation::ReadMail, "Read Mail", None)?;
        if page > 0 {
            target.url.query_pairs_mut().append_pair("page", &page.to_string());
        }
        Ok(target)
    }

    pub(crate) fn detail(
        &self,
        world: MoogleMailWorld,
        external_id: &str,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        if !valid_opaque_id(external_id) {
            return Err(self.configuration(MoogleMailOperation::Detail, world));
        }
        self.moogle_mail(world, MoogleMailOperation::Detail, "inbox", Some(external_id))
    }

    pub(crate) fn sent_page(
        &self,
        world: MoogleMailWorld,
        page: usize,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        let mut target = self.moogle_mail(world, MoogleMailOperation::Sent, "sent", None)?;
        if page > 0 {
            target.url.query_pairs_mut().append_pair("page", &page.to_string());
        }
        Ok(target)
    }

    pub(crate) fn sent_detail(
        &self,
        world: MoogleMailWorld,
        external_id: &str,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        if !valid_opaque_id(external_id) {
            return Err(self.configuration(MoogleMailOperation::SentDetail, world));
        }
        self.moogle_mail(world, MoogleMailOperation::SentDetail, "sent", Some(external_id))
    }

    pub(crate) fn compose(
        &self,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        self.moogle_mail(world, MoogleMailOperation::Compose, "new", None)
    }

    pub(crate) fn balance(
        &self,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        let mut url = self.endpoint.clone();
        url.set_path(world.path());
        url.query_pairs_mut().append_pair("s", "Bazaar").append_pair("ss", "is");
        Ok(MoogleMailTarget {
            operation: MoogleMailOperation::Balance,
            world,
            url,
            origin: self.origin.clone(),
        })
    }

    pub(crate) fn identity(&self, world: MoogleMailWorld) -> MoogleMailTarget {
        let mut url = self.endpoint.clone();
        url.set_path(world.path());
        url.query_pairs_mut().append_pair("s", "Character").append_pair("ss", "eq");
        MoogleMailTarget {
            operation: MoogleMailOperation::Identity,
            world,
            url,
            origin: self.origin.clone(),
        }
    }

    pub(crate) fn inventory(
        &self,
        world: MoogleMailWorld,
        category: super::MoogleMailInventoryCategory,
    ) -> MoogleMailTarget {
        let mut url = self.endpoint.clone();
        url.set_path(world.path());
        url.query_pairs_mut()
            .append_pair("s", "Bazaar")
            .append_pair("ss", "am")
            .append_pair("screen", "organize")
            .append_pair("filter", category.as_str());
        MoogleMailTarget {
            operation: MoogleMailOperation::Inventory,
            world,
            url,
            origin: self.origin.clone(),
        }
    }

    /// Armory 写入仅使用固定 organize 目标，不复用带分类的只读 URL。
    pub(crate) fn inventory_organize(&self, world: MoogleMailWorld) -> MoogleMailTarget {
        let mut url = self.endpoint.clone();
        url.set_path(world.path());
        url.query_pairs_mut()
            .append_pair("s", "Bazaar")
            .append_pair("ss", "am")
            .append_pair("screen", "organize");
        MoogleMailTarget {
            operation: MoogleMailOperation::InventoryOrganize,
            world,
            url,
            origin: self.origin.clone(),
        }
    }

    pub(crate) fn equipment(
        &self,
        world: MoogleMailWorld,
        equipment_id: &str,
        equipment_key: &MoogleMailEquipmentKey,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        if !valid_decimal_id(equipment_id) {
            return Err(self.configuration(MoogleMailOperation::Equipment, world));
        }
        let mut url = self.endpoint.clone();
        let path = format!(
            "{}/equip/{equipment_id}/{}",
            world.path().trim_end_matches('/'),
            equipment_key.expose()
        );
        url.set_path(&path);
        Ok(MoogleMailTarget {
            operation: MoogleMailOperation::Equipment,
            world,
            url,
            origin: self.origin.clone(),
        })
    }

    fn moogle_mail(
        &self,
        world: MoogleMailWorld,
        operation: MoogleMailOperation,
        filter: &str,
        external_id: Option<&str>,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        let mut url = self.endpoint.clone();
        url.set_path(world.path());
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("s", "Bazaar").append_pair("ss", "mm").append_pair("filter", filter);
            if let Some(external_id) = external_id {
                query.append_pair("mid", external_id);
            }
        }
        Ok(MoogleMailTarget { operation, world, url, origin: self.origin.clone() })
    }

    fn configuration(
        &self,
        operation: MoogleMailOperation,
        world: MoogleMailWorld,
    ) -> MoogleMailError {
        MoogleMailError::new(operation, world, MoogleMailErrorCategory::Configuration, false)
    }
}
