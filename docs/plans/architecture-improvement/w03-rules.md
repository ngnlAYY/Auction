# W3 规则上移

[计划入口](../architecture-improvement.md) · [执行须知](../architecture-improvement.md#2-执行须知) · [交付规范](../architecture-improvement.md#5-每个工作项的交付规范) · [进度](../architecture-improvement.md#7-进度) · [现状基线](baseline.md) · [决策记录](decisions.md)

| 项 | 内容 |
| --- | --- |
| 范围 | 规则上移：类型化快照、纯函数判定、时区无关证明、排期规则下沉 |
| 优先级 | P1 |
| 里程碑 | W3.7 随 M3（先于 W2）；W3.1–W3.3、W3.8 随 M4；W3.4–W3.6 随 M5 |
| 依赖 | W3.7 无依赖；其余批次依赖 W2 的领域类型与 W6.2 |
| 迁移 | 视批次（收窄数据库函数需迁移） |
| 估算 | 10–14 天，分批 |
| 关联缺陷 | D1（详见[已知缺陷](../architecture-improvement.md#已知缺陷)） |
| 关联决策 | —（详见[决策记录](decisions.md)） |

统一模式：

```text
storage：按锁序加载 → 类型化快照（结构体，而非 JSON Value）
application/domain：evaluate(snapshot) → 允许 / 阻断项闭集
storage：以快照版本做 CAS 写入；数据库函数保留为最后一道约束
```

权威划分：业务判定以 Rust 为准，数据库函数只保留不可绕过的最低约束；两边规则变化须在同一提交中同步，并以同一组样例证明结论一致。收窄或改写数据库函数需要新增迁移。各批次涉及的 SQL 侧 JSON 键谓词一并按 W12.6 提升为类型化列。

| 批次 | 目标 | 现位置 |
| --- | --- | --- |
| W3.1 | 人工 Sent 证明准入（两种准入、时间窗、资金占用） | `crates/auction-storage/src/database/sent_confirmation/snapshot.rs` |
| W3.2 | 装备损失处置准入 | `crates/auction-storage/src/database/equipment_loss/snapshot.rs` |
| W3.3 | 恢复阶段推进收敛为领域状态机 | `crates/auction-storage/src/database/recovery_scheduler/scheduler/advance.rs` |
| W3.4 | Admin 投递动作守卫（目击、退役、迁移表） | `crates/auction-storage/src/database/delivery_admin/commands.rs` |
| W3.5 | COD 实收资金化校验（结合 D1 结论） | `crates/auction-storage/src/database/settlement_funding/receipt.rs` |
| W3.6 | 履约 claim 一致性校验 | `crates/auction-storage/src/database/fulfillment_scheduler/claims.rs` |
| W3.7 | 行快照证明改为时区无关：显式列比较或 UTC 规范化时间，不依赖会话 `TimeZone`（提前到 M3，先于 W2） | `crates/auction-storage/src/database/legacy_self_proof/mod.rs`、`crates/auction-storage/src/database/equipment_loss/proof.rs` |
| W3.8 | 排期日历纯规则（早鸟期、赛季前场次、夏令时换算）从应用层 ports 下沉到领域 | `crates/auction-application/src/ports/scheduling/early_bird.rs`、`crates/auction-application/src/ports/scheduling/calendar.rs` |

每批验收：业务规则不再以 JSON 下标表达；Rust 判定与数据库函数在同一组样例上结论一致（以 W6.2 集成测试固化，切换前影子比对）；W3.7 在非 UTC 会话下结论不变；W3.8 的已冻结场次排期结果逐场不变。回退：每批经运行时开关切回存储层旧判定，直到后续版本删除旧路径。
