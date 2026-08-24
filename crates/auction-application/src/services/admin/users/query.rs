//! Admin Users 列表查询用例。

use super::AdminUserService;
use crate::{
    AdminUserError, AdminUserListRequest, AdminUserPage, AdminUserStore, ApplicationError,
};

impl<S, I, O> AdminUserService<S, I, O>
where
    S: AdminUserStore,
{
    /// 读取有界用户管理列表。
    pub async fn list(
        &self,
        request: AdminUserListRequest,
    ) -> Result<AdminUserPage, ApplicationError> {
        if !request.validate() {
            return Err(AdminUserError::InvalidRequest.into());
        }
        let page = self.store.list(request.clone()).await?;
        if !page.validate(&request) {
            return Err(AdminUserError::StoredResultCorrupt.into());
        }
        Ok(page)
    }
}
