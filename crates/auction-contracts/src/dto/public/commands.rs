//! commands 相关 DTO。

use serde::{Deserialize, Serialize};

use super::super::shared::deserialize_null_default;

/// 浏览器用于即时反馈的 1B 出价上限；新命令仍由领域层校验，HTTP 编译门禁保证两者一致。
pub const MAX_PUBLIC_BID_AMOUNT: i64 = 1_000_000_000;

/// Public 账号读取及写操作绑定页面用户；旧 v1 客户端可以省略此请求头。
pub const PUBLIC_EXPECTED_USER_ID_HEADER: &str = "x-auction-expected-user-id";

/// Public 普通竞价请求。命令 ID 必须由调用方生成并在重试中保持不变。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlaceBidRequest {
    /// 旧 Go 解码会把缺失或 `null` 字符串保留为零值，再由命令专属校验返回稳定错误。
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub command_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub amount: i64,
}

/// Public 代理授权请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateProxyAuthorizationRequest {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub command_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub maximum_amount: i64,
    /// 旧 v1 解码会把缺失或 `null` 字段投影为空字符串，再返回 `invalid_proxy_mode`。
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub mode: String,
}

/// Public 降低代理上限请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LowerProxyAuthorizationRequest {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub command_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub maximum_amount: i64,
}
