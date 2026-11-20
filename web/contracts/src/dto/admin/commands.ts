// commands 相关 DTO。

/** Admin 拍卖生命周期命令；`command_id`、`expected_state_version` 缺失或为 `null` 时
 * 按零值处理。 */
export interface AdminAuctionLifecycleRequest {
  command_id: string;
  expected_state_version: number;
  /** 后端在值为 `null` 时省略该键。 */
  reason?: string;
}

/** Admin draft lot 修改请求；动作字段保持旧 HTTP surface 的字符串契约。 */
export interface AdminLotUpdateRequest {
  command_id: string;
  expected_state_version: number;
  action: string;
  /** 后端在值为 `null` 时省略该键。 */
  start_price?: number;
}

/** Admin 邮箱轮询开关请求；`command_id` 为 UUID。 */
export interface AdminMailPollingRequest {
  command_id: string;
  mailbox_key: string;
  paused: boolean;
  expected_state_version: number;
}

/** Admin 立即触发邮箱轮询请求；`command_id` 为 UUID。 */
export interface AdminMailPollTriggerRequest {
  command_id: string;
}
