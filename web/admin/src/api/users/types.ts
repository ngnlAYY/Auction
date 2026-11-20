// Admin 用户 API 的浏览器侧类型；wire 字段保持 snake_case，列表行合并基础用户与摘要。

import type {
  AdminUserBlacklistHistoryResponse,
  AdminUserPaginationResponse,
} from "@auction/contracts/dto/admin/users";

/** 用户列表行的浏览器投影；避免 UI 传播基础用户与黑名单摘要的 wire 嵌套。 */
export interface AdminUser {
  readonly id: string;
  readonly username: string;
  readonly forum_account_id: string | null;
  readonly status: string;
  readonly key_status: string;
  readonly state_version: number;
  readonly created_at: string;
  readonly updated_at: string;
  readonly blacklist_count: number;
  readonly blacklist_history: readonly AdminUserBlacklistHistoryResponse[];
}

/** 用户列表的一页安全投影与分页回显。 */
export interface AdminUserPage {
  readonly users: readonly AdminUser[];
  readonly pagination: AdminUserPaginationResponse;
}

/** 用户列表的状态筛选闭集；`all` 是默认值，不写地址栏也不发给服务端。 */
export type AdminUserStatusFilter = "all" | "active" | "blacklisted";

/** 用户写命令的浏览器结果；基础用户 wire 响应在边界转换为列表行投影。
 * TS 没有 Rust `Debug` 的脱敏：`generated_key` 是可枚举的原值，禁止记录、打印或持久化整个结果；
 * 需要调试时只输出 `user` 投影与是否生成 Key（`generated_key !== null`），不输出 Key 本身。 */
export interface AdminUserMutationResult {
  readonly user: AdminUser;
  readonly generated_key: string | null;
}
