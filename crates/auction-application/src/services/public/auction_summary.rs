//! 已结算场次摘要的按 ID 读取与身份校验。

use uuid::Uuid;

use crate::errors::ApplicationError;
use crate::ports::core::RepositoryError;
use crate::ports::public::{PublicAuctionSummaryReader, SettledAuctionSummary};

/// 查询指定场次的公开摘要，拒绝与请求身份或基本目录字段不符的存储投影。
pub async fn settled_auction_summary(
    reader: &dyn PublicAuctionSummaryReader,
    auction_id: Uuid,
) -> Result<Option<SettledAuctionSummary>, ApplicationError> {
    let summary = reader.read_settled_auction_summary(auction_id).await?;
    // 可长期缓存的摘要必须对应请求的场次，损坏投影失败关闭。
    if summary.as_ref().is_some_and(|summary| {
        summary.id != auction_id
            || summary.id.is_nil()
            || summary.sequence <= 0
            || !auction_domain::valid_text(&summary.world_id, 64)
    }) {
        return Err(RepositoryError::Failed("公开结算场次摘要无效".to_owned()).into());
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use uuid::Uuid;

    use super::settled_auction_summary;
    use crate::errors::ApplicationError;
    use crate::ports::core::ApplicationFuture;
    use crate::ports::public::{PublicAuctionSummaryReader, SettledAuctionSummary};

    struct FixedSummary(Option<SettledAuctionSummary>);

    impl PublicAuctionSummaryReader for FixedSummary {
        fn read_settled_auction_summary(
            &self,
            _auction_id: Uuid,
        ) -> ApplicationFuture<'_, Option<SettledAuctionSummary>> {
            Box::pin(async { Ok(self.0.clone()) })
        }
    }

    #[test]
    fn settled_auction_summary_requires_the_requested_identity() {
        let test = pin!(async {
            let id = Uuid::from_u128(1);
            let summary = SettledAuctionSummary {
                id,
                world_id: "main".to_owned(),
                sequence: 5,
                effective_end_at: None,
            };
            let reader = FixedSummary(Some(summary.clone()));
            assert_eq!(settled_auction_summary(&reader, id).await.unwrap(), Some(summary));
            assert!(matches!(
                settled_auction_summary(&reader, Uuid::from_u128(2)).await,
                Err(ApplicationError::Repository(_))
            ));
            assert_eq!(settled_auction_summary(&FixedSummary(None), id).await.unwrap(), None);
        });
        assert_eq!(test.poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(()));
    }
}
