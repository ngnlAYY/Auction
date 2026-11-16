//! Admin dashboard 的响应投影与查询参数。
//!
//! 缺省响应是已发布契约：字段闭集与口径保持基线，已打开的旧页面按闭集解码。等待领取相关的
//! 新字段与口径只在新页面显式请求 `include=collection_wait` 时输出，兼容规则见
//! `crates/auction-contracts/README.md` 的“变更规则”。

use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};

use crate::errors::api_error;
use crate::query::parse_query;

/// 看板响应投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DashboardProjection {
    /// 已发布的缺省响应：履约分区把已寄出等待领取的义务仍计入 `pending` 与积压，不评估长期
    /// 未领取；对账分区只给缺失件数与待付缺口。字段闭集与口径同基线。
    Baseline,
    /// `include=collection_wait`：履约分区按等待领取口径（等待领取不计积压，长期未领取超期
    /// 件数大于 0 时降级），追加等待领取分组与长期未领取清单；对账分区追加在途字段。
    CollectionWait,
}

/// 解析看板查询参数：只接受可选的 `include=collection_wait`。未登记的键或重复出现为
/// 400 `invalid_query`（查询参数无效）；`include` 的其他取值（含空值、大小写不同与逗号列表）
/// 同为 400 `invalid_query`（附加字段无效），不静默退回缺省投影。
pub(crate) fn dashboard_projection(uri: &Uri) -> Result<DashboardProjection, Box<Response>> {
    let mut query = parse_query(uri, &["include"]).map_err(|()| {
        Box::new(
            api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response(),
        )
    })?;
    match query.remove("include").as_deref() {
        None => Ok(DashboardProjection::Baseline),
        Some("collection_wait") => Ok(DashboardProjection::CollectionWait),
        Some(_) => Err(Box::new(
            api_error(StatusCode::BAD_REQUEST, "invalid_query", "附加字段无效").into_response(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::http::{StatusCode, Uri};
    use serde_json::Value;

    use super::{DashboardProjection, dashboard_projection};

    async fn parse(query: &str) -> Result<DashboardProjection, (StatusCode, Value)> {
        let uri: Uri = format!("/api/v1/admin/dashboard{query}").parse().expect("测试 URI");
        match dashboard_projection(&uri) {
            Ok(projection) => Ok(projection),
            Err(response) => {
                let status = response.status();
                let bytes =
                    to_bytes(response.into_body(), usize::MAX).await.expect("读取错误响应体");
                Err((status, serde_json::from_slice(&bytes).expect("错误响应是 JSON")))
            }
        }
    }

    /// 不带查询参数是缺省投影；`include=collection_wait` 是等待领取扩展投影。
    #[tokio::test]
    async fn default_and_collection_wait_projections() {
        assert_eq!(parse("").await, Ok(DashboardProjection::Baseline));
        assert_eq!(parse("?").await, Ok(DashboardProjection::Baseline));
        assert_eq!(
            parse("?include=collection_wait").await,
            Ok(DashboardProjection::CollectionWait)
        );
    }

    /// 未知取值、空值、大小写不同与逗号列表为“附加字段无效”；重复出现或未登记的键为“查询参数
    /// 无效”，与 Public `include=` 一致，都不静默退回缺省投影。
    #[tokio::test]
    async fn unknown_or_repeated_include_fails_closed() {
        for (query, message) in [
            ("?include=", "附加字段无效"),
            ("?include=COLLECTION_WAIT", "附加字段无效"),
            ("?include=in_transit", "附加字段无效"),
            ("?include=collection_wait,in_transit", "附加字段无效"),
            ("?include=collection_wait&include=collection_wait", "查询参数无效"),
            ("?poll=30", "查询参数无效"),
        ] {
            let (status, body) = parse(query).await.expect_err(query);
            assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
            assert_eq!(body["code"], "invalid_query", "{query}");
            assert_eq!(body["message"], message, "{query}");
        }
    }
}
