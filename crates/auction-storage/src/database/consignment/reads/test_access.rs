//! PG18 测试对生产查询取执行计划，并核对列表快照。

use auction_application::consignment::{
    AdminListingQuery, ConsignmentError, PrivateListingQuery, PublicListingQuery,
};

use crate::database::DatabaseStore;
use crate::database::consignment::reads::{catalog, page_order, private, snapshot};

impl DatabaseStore {
    /// 在生产计数和取页查询之间等待夹具提交另一连接的变更。
    #[doc(hidden)]
    pub async fn test_consignment_public_snapshot(
        &self,
        query: PublicListingQuery,
        between: impl AsyncFnOnce(u64),
    ) -> Result<(u64, usize), ConsignmentError> {
        let page = catalog::read_page(self, query, between).await?;
        Ok((page.total, page.items.len()))
    }

    /// 在夹具连接上测量生产列表与计数查询。
    #[doc(hidden)]
    pub async fn test_consignment_read_plans(
        &self,
        public: &PublicListingQuery,
        private: AdminListingQuery,
    ) -> Result<Vec<(&'static str, Vec<String>)>, auction_application::consignment::ConsignmentError>
    {
        let (mut tx, as_of) = snapshot(self, true).await?;
        let mut public_page = catalog::list_query(
            &format!("EXPLAIN (ANALYZE, BUFFERS) {}", catalog::page_select()),
            as_of,
            public,
        );
        page_order(&mut public_page, public.window);
        let public_count =
            catalog::list_query("EXPLAIN (ANALYZE, BUFFERS) SELECT count(*)", as_of, public);
        let private_page = private::list_query(
            as_of,
            private.listings,
            private.seller_user_id,
            false,
            true,
            "EXPLAIN (ANALYZE, BUFFERS) ",
        );
        let owned_count = private::list_query(
            as_of,
            private.listings,
            private.seller_user_id,
            false,
            false,
            "EXPLAIN (ANALYZE, BUFFERS) ",
        );
        let admin =
            PrivateListingQuery { world: public.world, mode: public.mode, ..private.listings };
        let admin_page =
            private::list_query(as_of, admin, None, true, true, "EXPLAIN (ANALYZE, BUFFERS) ");
        let admin_unfiltered = private::list_query(
            as_of,
            private.listings,
            None,
            true,
            true,
            "EXPLAIN (ANALYZE, BUFFERS) ",
        );
        let mut results = Vec::new();
        for (name, mut sql) in [
            ("public_page", public_page),
            ("public_count", public_count),
            ("owned_page", private_page),
            ("owned_count", owned_count),
            ("admin_filtered", admin_page),
            ("admin_unfiltered", admin_unfiltered),
        ] {
            let plan =
                sql.build_query_scalar().fetch_all(&mut *tx).await.map_err(super::storage_error)?;
            results.push((name, plan));
        }
        tx.commit().await.map_err(super::storage_error)?;
        Ok(results)
    }
}
