//! Admin 凭据安全投影查询。

use super::AdminCredentialService;
use crate::{
    AdminCredentialError, AdminCredentialListRequest, AdminCredentialPage, AdminCredentialStore,
    ApplicationError,
};

impl<S, E> AdminCredentialService<S, E>
where
    S: AdminCredentialStore,
{
    /// 读取有界安全投影列表。
    pub async fn list(
        &self,
        request: AdminCredentialListRequest,
    ) -> Result<AdminCredentialPage, ApplicationError> {
        if !request.validate() {
            return Err(AdminCredentialError::InvalidRequest.into());
        }
        let page = self.store.list(request.clone()).await?;
        if !page.validate(&request) {
            return Err(AdminCredentialError::StoredResultCorrupt.into());
        }
        Ok(page)
    }
}
