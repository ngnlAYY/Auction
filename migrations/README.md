# 数据库迁移导航

[文档导航](../docs/README.md) · [PostgreSQL 迁移](postgres/README.md) · [SQLite 迁移](sqlite/README.md) · [迁移执行 owner](../crates/auction-storage/src/database/migrations)

只追加迁移；两个后端共享领域语义，不要求迁移序号或数据库能力对等。

`postgres/` 保存完整生产 schema 的迁移，`sqlite/` 保存 Lite 档所需的 schema 子集。
两个目录不要求文件或数据库对象一一对应；两边都存在的领域对象必须保持名称和语义一致，
SQLite 未实现的完整生产能力必须在 Lite 能力清单或部署说明中明确。迁移只允许追加，已经
发布的文件禁止修改。已支持竞价事务的锁、唯一约束、审计事件和 outbox 语义必须与
[`docs/architecture.md`](../docs/architecture.md) 一起评审。


## 完整档与 Lite

PostgreSQL `000001` 至 `000039` 已按新领域 owner 追加核心竞价、结算/履约、生命周期、
身份/外部命令、恢复/可观测性、论坛同步、MoogleMail 轮询、Final phase 调度、领先时间修正和
生命周期/operations alert/lot 结束时间数据库不变量、论坛未知写入的幂等人工再授权账本及
异世界 carrier dispatch 修正、v28 bid INSERT 在滚动升级期间遗漏有效结束时间列的兼容投影，
已证明 MoogleMail 退信的独立 operations alert signal 闭集、operations alert 的非敏感
启用状态与成功扫描水位、配置状态发布时间与扫描水位的独立时钟语义，以及 HV 战斗页占用时
保留凭据和最后成功余额的 `busy` recovery 状态、分类内 lot 编号、论坛固定 #1/#2 的
远端身份/内容版本/独立刷新租约、同一 world/provider 的单用户论坛账号归属、Preview intake
切换不变量、Admin 最近事件跨流读取索引，以及 FinalFast 同状态降额矩阵；
Admin A20 的全局唤醒由 Worker control 处理，不新增数据库触发字段；`000035` 仅为 PostgreSQL
`inbound_mail` 增加 Admin 审查所需的 nullable sender/subject 与脱敏 message_display 投影，SQLite 不跟随新增。
`000036` 仅为 PostgreSQL `inbound_mail` 增加装备失败诊断 nullable sidecar，SQLite 不跟随新增；该投影不参与
幂等、command digest、completion 或 cursor。
SQLite `000001` 至
`000013` 已提交共有 Lite 子集、轮次租约、用户管理、取消/生命周期元数据、Final phase fence、
领先时间、lot 结束时间 shape、分类内编号、论坛账号身份与 Admin 最近事件读取索引。两者只表示
当前 schema 能力，不能单独证明全部业务表面已经验收；外部数据库表也不能未经审计地整体复制到
任一后端。

PostgreSQL `000038` 仅转换未开始的流拍返还；PostgreSQL `000039` 与 SQLite `000013` 为轮次增加
nullable 本地排期来源，并只根据唯一且匹配的成功 Final 事实补齐遗留完成标记。旧排期来源不回填时区，
无法证明的轮次不自动修复；迁移的具体边界见[对象契约](../docs/contracts/schema.md#追加迁移与历史事实)。


## 迁移执行边界

- 分体部署由受保护的 `auctionctl migrate` 单独迁移，或由 `auctionctl deployment start/restart` 在严格停机检查后迁移再启动。
- 合并部署升级先停止 `auction-server`，再以同一账号执行 `auctionctl migrate-managed --config-dir <绝对目录>`，
  该命令复用既有 supervisor/子进程共享锁，不以分体 unit 状态代替合并进程停止证明。
- 单角色和合并服务普通启动不迁移；首次网页明确确认是合并入口进入专用空库初始化的例外，不能用于旧库接管。
  当前初始化未复用 `000005`/`000015` 历史兼容执行视图，详见[实现缺口](../docs/status.md#源码核对发现的实现缺口)。

具体参数、权限和失败处理见[部署手册](../ops/deploy/README.md)及[CLI 手册](../apps/auctionctl/README.md)。
迁移失败保留现场，不自动回滚，不修改已发布文件或 checksum。

## 验证边界

除非用户明确要求，迁移目录不编写或运行功能测试用例；迁移结构变更仍需通过编译检查、
静态检查和人工审阅确认。

PostgreSQL `000037` 与 SQLite `000012` 仅修正可由追加式 bid 证明的未结算 live lot
连续领先起点；已结算金额与审计保持不变。
