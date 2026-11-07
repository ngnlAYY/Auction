# W4 证据优先的 Admin 运维台

[docs/plans/architecture-improvement.md](../architecture-improvement.md) · [docs/plans/architecture-improvement.md#2-执行须知](../architecture-improvement.md#2-执行须知) · [docs/plans/architecture-improvement.md#5-每个工作项的交付规范](../architecture-improvement.md#5-每个工作项的交付规范) · [docs/plans/architecture-improvement.md#7-进度](../architecture-improvement.md#7-进度) · [docs/plans/architecture-improvement/baseline.md](baseline.md) · [docs/plans/architecture-improvement/decisions.md](decisions.md)

| 项 | 内容 |
| --- | --- |
| 范围 | 证据优先的 Admin 运维台、投递检索与玩家支持视图 |
| 优先级 | P1 |
| 里程碑 | M4 |
| 依赖 | W2、W1.4 |
| 迁移 | 视实现 |
| 估算 | 9–12 天 |
| 关联缺陷 | —（详见[docs/plans/architecture-improvement.md#已知缺陷](../architecture-improvement.md#已知缺陷)） |
| 关联决策 | —（详见[docs/plans/architecture-improvement/decisions.md](decisions.md)） |

| 项 | 内容 |
| --- | --- |
| W4.1 | 投递详情 DTO：尝试历史、冻结期望摘要、Sent 观察与目击、领取状态、处置、关联义务与恢复 |
| W4.2 | 复用维护已写入的 `mailbox_sweep` 观察与 `sent_reference_sighting`，把 Sent 详情观察升级为可引用证据（摘要与回执，不含登录 key 正文） |
| W4.3 | Admin 发起人工 Sent 证明、Admin 失败更正、确认已处置：选择已采集证据 + 原因 + 稳定命令 ID，复用同一应用端口 |
| W4.4 | 对账对话框展示目击与候选；判失败保持闭集原因 |
| W4.5 | 投递检索：按收件人、参考编号（义务、投递、通知 ID）、模板与发送类别、世界和时间检索；列表显示模板、类别与退役或抑制原因 |
| W4.6 | 玩家支持视图：按用户或参考编号只读汇总出价、成交、义务、投递与通知（含退役与抑制原因），用于处理玩家咨询与核对冒充 Bot 的邮件 |

同步[docs/contracts/routes.md](../../contracts/routes.md)、五语言译文与前端 `Route`。验收：已领取货款、误判失败、结果未知通知、退件重试四类处置均可在 Admin 内闭环；按玩家报出的 Reference 一次检索即可定位对应义务、投递与通知。回退：新页面与接口为增量，旧对账入口保留到验收完成。
