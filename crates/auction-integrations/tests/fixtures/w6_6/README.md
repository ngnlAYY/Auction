# MoogleMail 发送成功页

- 采集：2026-11-06，真实 HV MoogleMail 测试发送（无资产、无 COD）。
- 原始来源：已登录真实账号的发送成功响应；不提交原始页面或 Cookie。
- 脱敏：玩家名、世界显示名、收件人名替换为 `Fixture*`；移除浏览器扩展 DOM；不含 token、MID 或凭据。
- 字节数：6254
- SHA-256：`f780259d75dab4a85fc2774c3724a58c0f352796694c5eda31f42692c687ac39`
- 用途：`prove_send_completed` 成功分支与 `MailDouble` 的 `WriteSucceeded`、`SlowWriteSucceeded`（POST 先延迟再返回同一页，供停机排空用例）。
