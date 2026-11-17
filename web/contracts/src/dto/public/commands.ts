// Public 命令请求 DTO 与浏览器共享常量。

/** 浏览器用于即时反馈的 1B 出价上限；新命令仍由领域层校验。 */
export const MAX_PUBLIC_BID_AMOUNT = 1_000_000_000;

/** Public 账号读取及写操作绑定页面用户；旧 v1 客户端可以省略此请求头。 */
export const PUBLIC_EXPECTED_USER_ID_HEADER = "x-auction-expected-user-id";

/** Public 普通竞价请求。命令 ID 必须由调用方生成并在重试中保持不变。 */
export interface PlaceBidRequest {
  /** 已发布契约沿用 Go `encoding/json` 的解码语义：缺失或 null 保留为零值，再由命令专属校验报错。 */
  command_id: string;
  amount: number;
}

/** Public 代理授权请求。 */
export interface CreateProxyAuthorizationRequest {
  command_id: string;
  maximum_amount: number;
  /** 旧 v1 解码会把缺失或 null 投影为空字符串，再返回 `invalid_proxy_mode`。 */
  mode: string;
}

/** Public 降低代理上限请求。 */
export interface LowerProxyAuthorizationRequest {
  command_id: string;
  maximum_amount: number;
}
