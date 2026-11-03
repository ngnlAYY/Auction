# W2 统一处置事实模型

[计划入口](../architecture-improvement.md) · [执行须知](../architecture-improvement.md#2-执行须知) · [交付规范](../architecture-improvement.md#5-每个工作项的交付规范) · [进度](../architecture-improvement.md#7-进度) · [现状基线](baseline.md) · [决策记录](decisions.md)

| 项 | 内容 |
| --- | --- |
| 范围 | 统一处置事实模型 |
| 优先级 | P1 |
| 里程碑 | M3，排在 W9.9 与 W3.7 之后 |
| 依赖 | W0、W3.7、W9.9；操作者字段按 W9.7 |
| 迁移 | 是 |
| 估算 | 6–8 天 |
| 关联缺陷 | —（详见[已知缺陷](../architecture-improvement.md#已知缺陷)） |
| 关联决策 | 46（详见[决策记录](decisions.md)） |

| 项 | 内容 |
| --- | --- |
| W2.1 领域 | `DeliveryDisposition` 闭集：`auto_sent_proof`、`operator_sent_confirmation`、`admin_failed`、`equipment_loss`、`legacy_self`、`preflight_retired`、`operator_acknowledged`；每类定义准入事实，并明确闭集覆盖“全部终态”还是“仅异常处置”，避免与正常成功路径重复计数 |
| W2.2 迁移 | 只追加表 `delivery_dispositions`（delivery、fence、kind、来源引用、操作者、原因、时间），追加触发器；来源引用按 kind 使用各自的可空外键并以 CHECK 约束“恰好一个非空”；表由单一应用端口拥有，其他用例经该端口写入；“操作者”按 W9.7（M2a 已落地）记录：Admin 为共享身份，CLI 附带系统用户，均如实标注来源；按 W9.9 的运行账号授权；同步保留政策与[数据库契约](../../contracts/database.md) |
| W2.3 回填 | 迁移内只回填能由既有事实精确证明的行，其余保持未处置；回填比较使用 W3.7 的时区无关形式 |
| W2.4 写入 | 各写者在原事务内经 W2.2 的端口追加处置：Admin 对账、装备损失、`confirm-sent`、提醒预检退役、legacy 修复 |
| W2.5 读侧 | 仪表盘、告警、Admin 列表改用“未处置”视图，先影子比对再删除 `reads.rs` 中的专用排除；新增“确认已处置”命令处理无业务影响的历史失败，领域层限定只适用于没有资金或资产影响的条目，必须填写原因 |
| W2.6 履约 | 履约终态同理（如装备损失后的线下补偿） |

验收：迁移前后各面板计数与当前排除口径逐项一致（影子期差异为零且各 kind 均有样本）；读侧不再含专用排除谓词。回退：读侧经运行时开关切回旧排除口径；处置表只追加，保留不删。
