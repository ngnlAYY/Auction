// Admin delivery outbox 页面装配。
// 页面状态、筛选器、表格/分页、确认对话框、详情页与展示辅助分别由子模块拥有；外部传输
// 结果仍由服务端命令端口证明，浏览器只提交安全投影中的 fencing version。

/** 投递列表、详情、Sent 证明和收件关联投递共用一个实体围栏。 */
export function deliveryCommandEntity(deliveryId: string): string {
  return `delivery:${deliveryId}`;
}
