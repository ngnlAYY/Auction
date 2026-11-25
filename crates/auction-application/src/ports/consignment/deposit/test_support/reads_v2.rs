//! 内存假实现的 v2 挂单读取端口。

use std::future;

use auction_domain::UserId;
use uuid::Uuid;

use super::FakeDepositStore;
use crate::ports::consignment::{
    AdminListingDetailV2, AdminListingQuery, AdminListingReadV2, AdminListingRecordV2,
    ConsignmentError, ConsignmentFuture, ListingReadPage, OwnedListingReadV2, PrivateListingQuery,
    PrivateListingV2,
};

// 内存假实现不持有挂单读模型；v2 挂单读取的行为由存储与 HTTP 层测试覆盖。
impl OwnedListingReadV2 for FakeDepositStore {
    fn list_owned_v2(
        &self,
        _seller_user_id: UserId,
        _query: PrivateListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<PrivateListingV2>> {
        Box::pin(future::ready(Err(ConsignmentError::Unavailable)))
    }

    fn owned_detail_v2(
        &self,
        _seller_user_id: UserId,
        _listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<PrivateListingV2>> {
        Box::pin(future::ready(Err(ConsignmentError::Unavailable)))
    }
}

impl AdminListingReadV2 for FakeDepositStore {
    fn list_admin_v2(
        &self,
        _query: AdminListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<AdminListingRecordV2>> {
        Box::pin(future::ready(Err(ConsignmentError::Unavailable)))
    }

    fn admin_detail_v2(
        &self,
        _listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<AdminListingDetailV2>> {
        Box::pin(future::ready(Err(ConsignmentError::Unavailable)))
    }
}
