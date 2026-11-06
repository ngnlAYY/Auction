# W11 资产对账、偿付能力与内部一致性巡检

[docs/plans/architecture-improvement.md](../architecture-improvement.md) · [docs/plans/architecture-improvement.md#2-执行须知](../architecture-improvement.md#2-执行须知) · [docs/plans/architecture-improvement.md#5-每个工作项的交付规范](../architecture-improvement.md#5-每个工作项的交付规范) · [docs/plans/architecture-improvement.md#7-进度](../architecture-improvement.md#7-进度) · [docs/plans/architecture-improvement/baseline.md](baseline.md) · [docs/plans/architecture-improvement/decisions.md](decisions.md)

| 项 | 内容 |
| --- | --- |
| 范围 | 资产对账、偿付能力与内部一致性巡检 |
| 优先级 | P1 |
| 里程碑 | W11.4 随 M2a；W11.1–W11.3 随 M5 |
| 依赖 | W11.1–W11.3 依赖 W1.3、W5；W11.2 另依赖 W14.8；W11.4 无依赖 |
| 迁移 | 是（报告事实） |
| 估算 | 5–6 天 |
| 关联缺陷 | —（详见[docs/plans/architecture-improvement.md#已知缺陷](../architecture-improvement.md#已知缺陷)） |
| 关联决策 | 11（详见[docs/plans/architecture-improvement/decisions.md](decisions.md)） |

| 项 | 内容 |
| --- | --- |
| W11.1 | 装备持有核对：由数据库推导 bot 应持装备（待退回、草稿、未寄出等），与 Armory 实际库存比对，输出差异报告 |
| W11.2 | 偿付能力报告：按待付卖方货款、待执行退款与邮资估算（W14.8）核对 Bot 的只读余额是否足够，不足时告警；仅记账模式下个人消费与 Bot 共用账号，不做余额漂移归因；只报告不改账 |
| W11.3 | 报告以追加事实保存，差异进入仪表盘与告警（W5） |
| W11.4 | 内部一致性巡检（提前到 M2a，作为 W2/W3 改造的安全网）：只读 SQL 不变量（lot 投影与出价事实、补偿池余额与流水、义务链完整性、投递与义务状态对应、realtime 流序号连续、已截止的 lot 不残留未执行的慢速代理调度），结果同样以追加事实保存；W10.6 迁移演练复用 |

约束：外部观察只读，按 W1 的最低优先级执行；报告不授权任何资金或资产动作。

验收：W11.4 在当前线上数据上运行，差异为零或每项差异有解释并进入处置；W11.1/W11.2 报告追加保存并进入仪表盘。回退：关闭巡检开关（只读，无数据回滚）。
