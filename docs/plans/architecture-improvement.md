# 架构改进计划

[docs/README.md](../README.md) · [docs/architecture.md](../architecture.md) · [docs/development.md](../development.md) · [ops/README.md](../../ops/README.md)

版本 v16（2026-11-08）。本计划只处理代码与工程体系，不包含线上数据处置；线上问题在各阶段完成并经用户批准部署后自然受益。本页是计划入口：目标与原则、执行须知、工作流与里程碑、交付规范、待决定事项与进度；每个工作流的工作项、验收与回退见各自的详情页，现状基线与决策记录见附录。正文中的仓库路径均以仓库根为起点；代码证据以“文件 + 符号名”引用，不写行号。

| 文档 | 内容 |
| --- | --- |
| 本页 | 目标与原则、执行须知、工作流总览与已知缺陷、里程碑、交付规范、仍待决定与用户前置动作、待办池、进度、术语 |
| 工作流详情（`docs/plans/architecture-improvement/w00-*.md` 至 `w14-*.md`） | 每个工作流一页：概要（优先级、里程碑、依赖、迁移、估算、关联缺陷与决策）、工作项、关键约束、验收与回退；从[工作流总览](#3-工作流总览)进入 |
| [docs/plans/architecture-improvement/baseline.md](architecture-improvement/baseline.md) | 制定计划时的代码与运行现状及证据位置 |
| [docs/plans/architecture-improvement/decisions.md](architecture-improvement/decisions.md) | 用户决策 1–83 与计划版本历史 |

## 1. 目标与原则

目标：在不削弱现有资金安全与审计保证的前提下，降低外部依赖导致的故障面、减少“例外谓词”式维护、让人工处置以证据为中心，并建立可重复的发布、备份与安全基线。

原则：

1. [AGENTS.md](../../AGENTS.md)的领域不变量全部保持：命令幂等、Serializable 关键事务、追加式事实、结果未知不重发、缓存不决定正确性、冻结策略不改写。
2. 每个交付批次一个阶段提交（依赖相连、一起交付的工作项合并，正文按工作项分节，决策 69），按“领域 → 用例/端口 → 存储/外部适配 → HTTP/Web/Worker”落地，同步契约与运维文档。
3. 默认门禁仍为只做静态验证的 `just verify`；行为变更运行受影响范围的已批准 W6 测试；其他行为验证、数据删除与线上部署需要用户逐项批准（见[docs/plans/architecture-improvement/decisions.md](architecture-improvement/decisions.md)）。
4. 先可观测、再改行为：每个改变运行行为的阶段，都先有能证明改进效果的指标，并且指标有地方存储和查看（W0.10）。
5. 无预发环境（决策 9），因此改变运行行为的工作项必须带运行时开关与旧路径回退（W7.7 落地后开关切换无需重启，不受 W7.5 时段门禁阻挡）；涉及判定或计数口径的改动先影子运行（规范见[交付规范](#5-每个工作项的交付规范)），差异归零后再切换。
6. 每个工作项写明可度量的验收标准与回退方式。W7.8a 随一版部署之后，只含 `compatible` 迁移的版本可以换回旧程序；含 `breaking` 迁移的版本只能按[ops/deploy/RECOVERY.md](../../ops/deploy/RECOVERY.md)回退（见 W7.2、W7.8a）。
7. 每项结构或错误处理改进都配套防回退门禁（W13），避免改进成果被后续提交抵消。

## 2. 执行须知

本计划由执行者（人或智能体）逐项推进。开始任何工作项前，先读完本节、[AGENTS.md](../../AGENTS.md)与对应的工作流详情页（链接见[工作流总览](#3-工作流总览)）。

**选择与认领**

1. 按[里程碑](#4-推进顺序与里程碑)顺序推进，里程碑内按表中顺序，并遵守总览与详情页写明的依赖；满足条件的已知缺陷按第 4 节单独交付，不等待里程碑。
2. 开始前在[进度](#7-进度)把该项标为“进行中”并注明执行者与分支；认领与进度更新不单独提交，随所在批次的阶段提交一并提交（决策 69）。同一工作项同一时间只由一个执行者负责。多个执行者并行时各用独立分支或 worktree（决策 70），进度表冲突按最新状态合并。
3. 工作项被[仍待决定](#仍待决定)的事项或[用户前置动作](#用户前置动作)阻塞时，停下向用户说明，不自行假设结论。

**范围**

4. 计划范围自 v13 起冻结：工作项内容、验收与回退以详情页为准。执行中的新发现（缺陷、遗漏、改进想法）写入[待办池](#待办池)，不直接扩写工作项；里程碑结束时由用户决定是否纳入。
5. 详情页与[docs/plans/architecture-improvement/baseline.md](architecture-improvement/baseline.md)反映制定计划时的代码；实施前先核对当前代码，不符时以代码为准，并在交付说明中记录差异。只做工作项要求的改动，需要偏离详情页时先征得用户同意。

**必须由用户执行或批准**

6. 线上部署、线上写操作、GitHub 远端设置、R2 与主机配置由用户执行；执行者负责构建、给出命令与只读核验步骤。部署使用 CI 在 `main` 上产出的候选制品（决策 51）。
7. 线上只读查询（如 W0.12）需要用户提供访问并逐次同意：数据库查询放在只读事务内；查看游戏页面时不打开 Bot 收件箱中的未读邮件（详情读取会把它移出未读，干扰 Worker 收件），也不打开登录 key 邮件。
8. 已批准的测试类别只有 W6.1–W6.4 与 W6.6；其他测试、启动服务做行为验证、数据删除与破坏性数据库操作都要先征得用户同意。
9. 发布冻结期间只交付修复（决策 57）；外部心跳与告警服务、MoogleMail 运维告警暂不启用（决策 50、58），不要提前实现启用。

**交付与停顿**

10. 每个工作项按[交付规范](#5-每个工作项的交付规范)完成：写明保护的不变量与契约变化，同步契约与文档，`just verify` 通过，一个主题一个提交，更新进度表；玩家可见的变化附 Admin“规则内容”五语言更新稿。
11. 每个里程碑结束时停下，向用户汇报变更、门禁结果与部署建议，由用户决定是否部署；部署后按验收标准核对，并在进度表记录部署版本与结论。

## 3. 工作流总览

| 编号 | 工作流 | 优先级 | 依赖 | 迁移 | 估算 |
| --- | --- | --- | --- | --- | --- |
| [docs/plans/architecture-improvement/w00-errors-metrics.md](architecture-improvement/w00-errors-metrics.md) | 错误分类、指标与采集路径、用户可见 SLO、已知缺陷诊断、待核实事实、就绪探针、影子运行框架、浏览器错误信标 | P0（W0.15、W0.16 为 P1） | W0.14 先于 W0.6 | 是（W0.13 索引 077；其余否） | 10–13 天 |
| [docs/plans/architecture-improvement/w01-external-account.md](architecture-improvement/w01-external-account.md) | 外部账号调度：快速限界、熔断、三层争用统一调度、访问收口、主动暂停、增量扫描、证据退避、论坛对齐与增量导入 | P0 | W0；W1.11 依赖 W0.5；W1.1、W1.4、W1.11 先有 W6.3；模拟类验收依赖 W6.6；W1.10 依赖 W1.3 | 是（暂停开关、扫描水位、论坛游标） | 15–19 天 |
| [docs/plans/architecture-improvement/w02-dispositions.md](architecture-improvement/w02-dispositions.md) | 统一处置事实模型 | P1 | W0、W3.7、W9.9；操作者字段按 W9.7 | 是 | 6–8 天 |
| [docs/plans/architecture-improvement/w03-rules.md](architecture-improvement/w03-rules.md) | 规则上移：类型化快照、纯函数判定、时区无关证明、排期规则下沉 | P1 | W3.7 无依赖；其余批次依赖 W2 的领域类型与 W6.2 | 视批次（收窄数据库函数需迁移） | 10–14 天，分批 |
| [docs/plans/architecture-improvement/w04-admin-console.md](architecture-improvement/w04-admin-console.md) | 证据优先的 Admin 运维台、投递检索与玩家支持视图 | P1 | W2、W1.4 | 视实现 | 9–12 天 |
| [docs/plans/architecture-improvement/w05-alerting.md](architecture-improvement/w05-alerting.md) | 告警通道入口、告警来源补齐、外部存活检查、仪表盘口径与告警分级、处置手册、告警台 | P1（W5.1、W5.6 暂缓，决策 50） | W5.1、W5.5–W5.7 依赖 W0；W5.2–W5.4、W5.8 依赖 W2 | 是（W5.5 扩展告警闭集，W5.8 确认与静默） | 8–11 天 |
| [docs/plans/architecture-improvement/w06-verification.md](architecture-improvement/w06-verification.md) | 验证体系（W6.1–W6.4、W6.6 已批准） | P1 | W6.1 中各算法测试随对应工作项交付；W6.6 依赖 W6.3 | 否 | 16–22 天 |
| [docs/plans/architecture-improvement/w07-release.md](architecture-improvement/w07-release.md) | 发布流水线与环境：时段门禁、运行时开关、版本偏差、故障域隔离、迁移兼容分级、运行参数热读取、停机排空 | P2（W7.1a、W7.5、W7.7、W7.8a、W7.11 为 P0；W7.9、W7.10 为 P1） | W7.2 依赖 W10.6；W7.9 依赖 W0.12；W7.8 其余依赖 W6.2、W6.4 | 是（运行时开关、迁移兼容登记） | 12–16 天 |
| [docs/plans/architecture-improvement/w08-consolidation.md](architecture-improvement/w08-consolidation.md) | 规模收敛评估（只出决策记录，放在最后） | P2 | 其余工作流完成后 | 否 | 2–3 天 |
| [docs/plans/architecture-improvement/w09-security.md](architecture-improvement/w09-security.md) | 安全加固：密钥环、已知取舍复核、Admin 登录防护、主机加固、操作者身份、数据库最小权限、登录 key 自助轮换、入站与论坛滥用防护 | P2（W9.5、W9.7–W9.11 为 P1） | W9.9 先于 W2；W9.11 与 W9.10 共用计数 | 是（密钥环、运行账号授权、回信预算计数） | 11–15 天 |
| [docs/plans/architecture-improvement/w10-data-backup.md](architecture-improvement/w10-data-backup.md) | 数据生命周期与备份：定时与异机备份、迁移演练、WAL 归档、RPO/RTO、恢复演练、按政策清理、PostgreSQL 服务端基线、恢复后外部核对 | P0（W10.1、W10.2、W10.6），其余 P1 | W10.7 依赖 W10.2、W10.8；W10.9 依赖 W1.9、W1.10 | 视政策 | 13–17 天 |
| [docs/plans/architecture-improvement/w11-reconciliation.md](architecture-improvement/w11-reconciliation.md) | 资产对账、偿付能力与内部一致性巡检 | P1 | W11.1–W11.3 依赖 W1.3、W5；W11.2 另依赖 W14.8；W11.4 无依赖 | 是（报告事实） | 5–6 天 |
| [docs/plans/architecture-improvement/w12-database-hotspots.md](architecture-improvement/w12-database-hotspots.md) | 数据库热点与读路径：同场出价序号热点、重试退避、读路径保护、连接配额、JSON 键列化、出价事务内的唤醒通知 | P1（W12.1、W12.4、W12.5），其余 P2 | W0.6、W0.9、W0.13、W7.7 | 是（W12.1 视方案、W12.6） | 8–11 天 |
| [docs/plans/architecture-improvement/w13-regression-gates.md](architecture-improvement/w13-regression-gates.md) | 回归门禁：计数棘轮、panic 与溢出防护、文档链接检查、定时依赖审计、依赖升级节奏 | P1 | — | 否 | 3–4 天 |
| [docs/plans/architecture-improvement/w14-notifications.md](architecture-improvement/w14-notifications.md) | 通知与提醒：通知目录与模板、内容修正、新增提醒、论坛 notify 改为订阅、普通出价自动标记与渠道前置、付款提醒联动、通知指标与邮资、收件人异常、慢速代理提醒时机 | P1 | W14.8 先于 W14.1，W14.1 先于 W14.2–W14.4；W14.3 依赖 W1.3；W14.5 依赖 W7.6；W14.11 与 W1.7 同批 | 是（通知事实与订阅上限、偏好字段、待定提醒） | 12–17 天 |

估算为单人连续工作日，含文档同步与静态门禁，不含线上部署与观察；合计约 140–188 天（v10 为 117–156 天，增量来自 W14 与各工作流补遗；v12 取消 W14.7）。影子运行与部署后观察另占日历时间（见[交付规范](#5-每个工作项的交付规范)）。

### 已知缺陷

| 编号 | 缺陷 | 处理 |
| --- | --- | --- |
| D1 | Admin 失败更正后的 COD 实收资金化每轮返回未持久化错误，付款义务停在 `enqueued`，卖方货款保持阻塞；数据库无报错，属于 Rust 侧拒绝，具体原因被应用层 `Err(_)` 丢弃 | W0.7：先让错误可分类，再定位修复；修复前保持人工证明事实不变 |
| D2 | `68c1b8df` 让 Read Mail 观察真实翻页后，与 Sent 维护共享预算并持有 `incoming` 读锁，维护每轮超时，Sent 找回、草稿清理与目击停摆 | W1.0 快速限界，作为 M1 首项单独交付（决策 46）；W1.3、W1.4 根治 |
| D3 | 潜在：发送前 Sent 完整基线受 `maximum_pages ≤ 100` 限制，Sent 若只增不减，超过上限后所有发送失败关闭 | W0.12 核实增速与 HV 清理行为；W1.4 改为 MID 水位前缀扫描；若核实结果显示临近上限则提前单独交付 |
| D4 | 出队让出价被超提醒优先，与 W1.7“通知最低优先级”的方向相反；提醒发送资格不检查 lot 是否已截止 | 已决定（决策 14，决策 61 修订）：运维告警 > 账号类回信 > 按 lot 有效截止排序的出价被超提醒 > 其他邮件；截止后尚未发送的提醒退役（MM 与论坛 PM 队列都适用，PM 的发送资格同样不检查截止）。在 W1.3、W1.7（M2b）实现；W0.13 的“提醒送达时距截止余量”用于验收 |
| D5 | 同场出价序号热点：每笔出价在 Serializable 事务中递增同一 `realtime_streams` 行，同场重叠出价必有一方序列化失败，立即重试 3 次后可能耗尽，玩家收到暂不可用 | W0.6 先按事务 owner 量化冲突与耗尽；W12.1 根治；若 Final 阶段耗尽次数非零，W12.1 提前单独交付 |
| D6 | 论坛回帖：按导入时刻判定截止（轮询 300 秒），截止前的回帖可能被拒或软截止从导入时起算；主题超过 `maximum_topic_pages` 后整场导入失败关闭 | 已决定（决策 15）：维持按导入时刻判定，玩家可见说明（Admin 发布的五语言规则文档与 `docs/guide`）随 M1 先行；W1.11 增量导入与 Final 加速；若 W0.12 显示已有主题临近页数上限，增量导入提前单独交付 |
| D7 | 一次审计写入失败即永久未就绪：计数不区分可重试冲突、连接中断与超时，进程只能重启恢复；W0.6 的超时会放大该问题 | W0.14（决策 32）：按 SQLSTATE 分类，须先于 W0.6 上线 |
| D8 | 停机排空 10 秒短于在途外部写入：越过 sending 围栏后被中断的发送、发帖与私信成为结果未知 | W7.11（决策 33）：关键段独立排空预算并同步 `TimeoutStopSec` |
| D9 | 付款期限从结算建立恢复记录起算，逾期提醒在期限到达时创建，两者都不检查付款邮件（COD 或载体）是否已寄出：邮件积压、账号忙或人工对账期间，买家可能从未收到付款邮件，却收到“请查看原付款邮件”的逾期提醒；赛季前场次只有 24 小时，影响最大 | 已决定（决策 62）：W14.6（M2b）把逾期提醒延后到付款邮件有发送证明之后，并告警“付款邮件未寄出但已逾期”；不改冻结期限，不新增恢复策略版本 |
| D10 | 论坛 `notify` 每行、每个 lot 代码各寄一封 MoogleMail，没有去重或上限，回执正文不含 lot、金额或结果；一条回帖即可让 Bot 账号消耗大量请求预算与邮资 | 已决定（决策 60）：W14.4（M2b）把 `notify` 改为订阅该 lot 的被超提醒，不再寄回执；W0.12 若发现大量使用或滥用，单独提前交付 |
| D11 | 选择论坛私信渠道而未绑定论坛账号的用户，被超提醒在出价事务内即被抑制（`recipient_missing`），偏好接口与页面都不提示；论坛同步关闭时 PM 渠道仍可选 | W14.5（M2b） |

## 4. 推进顺序与里程碑

| 里程碑 | 内容 | 完成标志 |
| --- | --- | --- |
| M1 | W1.0、W10.1、W10.2、W7.1a、W0.1–W0.14、W7.5、W7.7、W7.8a、W7.11、W10.6、W13.1–W13.5（W13.3b 延后到 M2a） | 维护不再超时；定时备份与异机副本运行，数据密钥已离线托管，端到端恢复演练完成一次；`main` 成为部署基线，部署改用 CI 制品；错误、外部请求、HTTP、SLO、主机与数据库可度量且可采集；D1 原因可读，D5 冲突率有基线；就绪探针不再被瞬时错误永久翻转；待核实事实写入基线；Final 阶段无法误停机，停机不再打断越过围栏的外部写入；运行时开关可用；此后只含兼容迁移的版本可换回旧程序；迁移演练流程可用；回归门禁、链接检查与定时审计生效 |
| M2a | W0.15、W13.3b、W6.3、W6.6、W6.1、W6.2、W6.4、W6.5、W9.5、W9.7、W10.8、W10.7、W11.4 | 测试与安全底座：回放语料、外部服务替身、测试入口、规范与 CI 测试作业可用；影子运行框架就绪；Admin 登录防护补齐；CLI 操作者可追溯；PostgreSQL 有配置基线，WAL 归档可做 14 天内的时间点恢复；一致性巡检可作为后续改造的安全网；逐 owner panic 计次随 W0.15 同批 |
| M2b | W1.1–W1.3、W1.6、W1.7、W1.9–W1.11、W7.9、W7.10、W9.8、W9.10、W9.11、W14.8、W14.1、W14.2、W14.4–W14.6、W14.9、W14.11 | 调度与玩家可见变化：熔断、统一调度与主动暂停生效，指标达标；Admin/CLI 不再直连账号；论坛增量导入消除页数悬崖；单角色故障不再拖垮 Public 与 Admin；发送节奏与预算无需重启即可调整；提醒按截止排序且截止后退役；玩家会话满额可自动腾出名额，玩家可自助轮换登录 key；通知正文能识别装备、金额与期限，论坛 notify 与说明信有上限，PM 渠道不再静默失效，逾期提醒不早于付款邮件 |
| M3 | W9.9、W3.7、W1.4、W1.5、W1.8、W2、W0.16、W7.6 | 服务改用无 DDL 的运行账号；时区无关证明先行；增量扫描消除 Sent 悬崖；处置模型落地，仪表盘无专用排除；浏览器错误可计数，部署后旧页面提示刷新 |
| M4 | W5.2–W5.5、W5.7、W5.8、W3.1–W3.3、W3.8、W4、W12.1、W12.3、W12.4、W12.5、W12.8、W14.3 | 告警来源齐全、分级合理、处置手册就绪，告警台可确认与静默；运维台闭环，可按 Reference 检索并查看玩家支持视图；核心准入上移；同场出价热点消除，读路径与关键循环受保护；投稿回执、卖方成交汇总与换季提醒上线 |
| M5 | W3 其余、W10.0、W10.3–W10.5、W10.9、W11.1–W11.3、W9.3、W9.4、W9.6、W12.6 | 恢复演练、清理与对账自动化，恢复后外部核对有工具，偿付能力可报告，密钥环与主机加固完成 |
| M6 | W7.1、W7.2、W7.3、W7.8、W12.2、W12.7、W13.6，最后 W8 | 发布流水线可用，兼容声明有测试验证，热路径按指标调整，依赖升级有固定节奏，最后完成部署形态与结构评估 |

里程碑内按表中顺序推进（决策 46、54）。M1 的前三项先行：W1.0（D2）、W10.1 与 W10.2（含 bucket lock、数据密钥托管与端到端恢复演练）各自完成即单独交付，W7.1a 只需用户完成远端设置；随后 W0.1 → W0.7（D1）、W0.14（D7，须先于 W0.6 的超时）、W7.11（D8），再完成 M1 其余各项。W13.3b 按决策 75 延后到 M2a，与 W0.15 同批。M2a 先交付 W6.3 与 W6.6，M2b 才开始依赖它们的 W1.1、W1.11；W10.8 先于 W10.7；W9.10 与 W9.8 同批，W9.11 与 W9.10 共用计数；M2b 中 W14.8 先于 W14.1，W14.1 先于 W14.2、W14.4。M3 以 W9.9 开头，先于 W2 新建的表。W5.1、W5.6 暂缓（决策 50），用户选定外部服务后排入当时的里程碑。

发布冻结（决策 57）：从赛季前场次开拍起，到首场早鸟场次结算完毕，只部署修复，不上线行为变更或含 `breaking` 迁移的版本；下一次为 2027-05-01 换季前后。W7.5 的时段门禁照常生效。

每个里程碑结束时停下来：汇报变更、门禁结果与部署建议，由用户决定是否部署。以下缺陷一经满足条件即单独交付，不等待里程碑：D2 的 W1.0 完成即交付；D1 修复一经定位；D3 若经 W0.12 核实临近上限，W1.4 的发送基线部分；D5 若 Final 阶段耗尽次数非零，W12.1；D6 若已有主题临近页数上限，W1.11 的增量导入部分；D10 若 W0.12 发现 `notify` 被大量使用或滥用，W14.4 的去重与上限。D6 现行语义的玩家可见说明（Admin 发布的五语言规则文档与 `docs/guide`）随 M1 先行。

## 5. 每个工作项的交付规范

1. 开始前 `git status --short`，确认基线分支，保留与任务无关的改动。
2. 先写清本项保护的不变量与契约变化（路由、JSON、事件、配置、数据库），以及验收标准与回退方式。
3. 改变运行行为的项带运行时开关并保留旧路径（W7.7 落地前用 TOML 开关，并在说明中注明切换需要重启、受 W7.5 约束）。判定或计数口径变化先影子运行（使用 W0.15 的共享框架）：差异以结构化日志与 `auction_shadow_mismatch_total{owner}`、`auction_shadow_evaluations_total{owner,branch}` 记录；切换条件为观察期覆盖至少一个包含 Final 与结算的完整场次周期、每个判定分支都有样本且差异为零（无样本的分支不算通过）；切换后在后续版本删除旧路径。
4. 实现后同步契约文档、模块 README、运维文档与配置示例；文档中的仓库路径以仓库根为起点，代码证据写符号名而非行号。玩家可见的变化除 `docs/guide` 外，还要在交付说明中给出 Admin“规则内容”五语言规则文档的更新稿，部署后由用户发布；界面文案同步五语言 `i18n/*.ftl`；新增或修改出站通知时同步 W14.1 的通知目录。
5. `cargo fmt --all` 与 `just verify` 通过；行为变更必须运行受影响范围的 `just test` / `just test-db`，已批准的 W6 测试随功能一起更新，入口与覆盖限制见[docs/development.md#测试](../development.md#测试)。涉及外部账号或论坛的行为先在 W6.6 替身上验证；涉及数据库的判断在本地 PostgreSQL 18 只读预览核对；含迁移的版本在交付说明中附 W10.6 迁移演练回执；W6.2 接入前匹配 0 项不能作为数据库验收。
6. 提交标题 `type(scope): summary`，正文写不变量与验证范围；一个交付批次一个阶段提交，批次内各工作项分节说明（决策 69）；W13 的计数基线随之下降时一并更新。
7. 在[进度](#7-进度)更新状态：本批次的行在提交号一栏写“随本提交”，下一批次提交时回填实际提交号。
8. 用户部署后，按验收标准核对线上指标，在进度表记录部署版本与验收结论；未达标时按回退方式处理并记录原因。

## 6. 待决定、前置动作与待办池

### 仍待决定

新的待决定事项出现时在此登记，并标注阻塞的工作项与最晚决定时间；决定后移入[docs/plans/architecture-improvement/decisions.md](architecture-improvement/decisions.md)。

| 事项 | 阻塞 | 最晚决定 |
| --- | --- | --- |
| SLO 目标值（出价 p99 延迟、冲突中止率、实时推送延迟、登录 key 邮件送达延迟、提醒距截止余量、Final 阶段回帖导入延迟），由我按 W0.13 基线提出（决策 56） | W1、W12 的验收 | M1 结束时 |
| W0.6 服务超时启用：`timeouts_enabled` 默认关闭；启用须 W0.14 部署并按决策 72 观察后三角色有效策略为 `classified`，候选值 acquire 2 秒、statement 5 秒、lock 1 秒；`idle_in_transaction_session_timeout` 非 0 时产生的 `25P03` 按决策 71 仍归 unknown、会粘滞阻断，是否启用或扩展白名单另定；预算中的 `superuser_reserved_connections`/`reserved_connections` 待线上只读核实 | W0.6 超时（D5 量化与分档不受阻） | W0.14 切换 `classified` 后 |
| 外部心跳与告警服务的选型与启用时间（决策 50），以及是否启用 MoogleMail 运维告警（决策 58，启用须先有 Bot 以外的收件角色） | W5.1、W5.6、备份打卡与 W5.5、W5.8 的主动通知；启用前没有任何主动告警，备份失败、Worker 崩溃与整机宕机都不会主动通知 | 无硬性期限 |

### 用户前置动作

以下动作需要用户在对应工作项交付前完成；执行者负责给出命令或步骤与只读核验。

| 动作 | 服务的工作项 | 时间 |
| --- | --- | --- |
| R2：建立 bucket，按日/月备份、WAL、基础备份前缀配置生命周期规则与 bucket lock（日 30 天、月 365 天、WAL 与基础备份 21 天），签发只限该 bucket 的对象读写令牌并写入敏感 TOML | W10.2、W10.7 | M1（部署前配置；异机启用需候选安装，WAL 与基础备份前缀在 M2a） |
| 把三组数据密钥与 `age` 私钥一起离线托管（密码管理器 + 离线副本），并参加一次端到端恢复演练 | W10.2 | M1 |
| 离线生成 `age` 密钥对：私钥存密码管理器与一份离线副本，公钥交给服务器 | W10.2 | M1 |
| 选定心跳/Webhook 服务：建立备份与 WAL 打卡检查、Worker 心跳检查与 Public `/health/ready` 的 HTTP 检查，报到地址写入敏感配置 | W10.1、W10.7、W5.1、W5.6 | 暂缓（决策 50） |
| 主机安装 Prometheus、node_exporter、postgres_exporter（只监听 127.0.0.1），配置 SSH 隧道查看 | W0.10 | M1（部署前安装服务；三角色 loopback 出口在候选安装后启用） |
| 安装并启用 `auction-backup.timer`，先执行一次手动 `backup create`/`backup verify` | W10.1 | M1（候选安装后；旧 CLI `f99a9d4e` 不含 `backup scheduled`） |
| GitHub：建立 `main`、设为默认分支并开启分支保护（必须通过 CI）；删除已合入的旧分支，关闭 `tokio-1.54.1` 与 `tungstenite-0.30.0` 两个 dependabot PR | W7.1a | M1（已完成 2026-11-04：`main` + 保护，旧分支删除，tokio PR #10 关闭；tungstenite 已不存在） |
| 提供 W0.12 线上只读核实所需的 SSH 与 psql 访问 | W0.12 | M1 |
| 应用 PostgreSQL 配置片段并按 W7.5 选择窗口重启 | W10.8、W10.7 | 首次部署之后（见上线顺序第 4 步） |
| 建立运行账号 `auction_runtime`、owner-only 敏感 TOML 与 `[database_roles] owner` 声明，三角色改用运行账号 | W9.9 | 首次部署前（`deploy`/`service start` 的严格预检硬性要求，2026-11-08 源码审计；步骤见[ops/README.md#首次部署-main-候选的上线顺序](../../ops/README.md#首次部署-main-候选的上线顺序)） |
| 在 Admin“规则内容”发布五语言规则更新 | D6、W1.7、W1.11、W9.8、W9.10、W9.11、W14 的玩家可见变化 | 随对应部署 |
| 启用 MoogleMail 运维告警时：准备 Bot 以外的 HV 角色作为告警收件人并写入配置 | W5.5、W5.8 | 暂缓（决策 58） |

执行顺序约束：R2、`age`、Prometheus/exporter 服务与 `auction_metrics` 账号可在候选部署前准备；`auction-backup.timer`、三角色 loopback metrics 出口及对应验收必须等 M1 候选（含 `backup scheduled` 与 W0.10）安装后执行。线上旧 CLI `f99a9d4e` 不具备这些子命令/出口，不能在旧程序上启用新 timer 或要求出口验收。

线上运维事项（不属于本计划）：`fetch_timeout_seconds` 维持现状；xilixilixi 邮件为日常交流、与拍卖无关，不重试，待 W2.5 的“确认已处置”收口。

### 待办池

执行中发现、尚未纳入计划的事项。每条写明来源工作项与建议归属；里程碑结束时由用户决定纳入、合并或放弃，纳入后移入对应详情页并在[docs/plans/architecture-improvement/decisions.md](architecture-improvement/decisions.md)登记。

| 登记日期 | 事项 | 来源 | 建议归属 | 状态 |
| --- | --- | --- | --- | --- |
| 2026-11-03 | 就绪探针瞬时白名单是否扩展到 `53300`（连接数耗尽）、`57P01/57P02/57P03/57P05`（服务器关闭、重启、崩溃恢复、空闲会话超时）、`25P03/25P04`（事务内空闲或事务超时）：这些错误在 PostgreSQL 计划重启（如 W10.8、W10.7）时可能出现，现按未知粘滞阻断，需重启进程恢复（与现状一致，无退步） | W0.14 | W0.14 | 已决定（决策 71），并入 W0.14 |
| 2026-11-03 | 有意改变判定的工作项（如 W0.14）如何满足交付规范第 3 条“影子差异为零”：W0.14 采用默认 `shadow` 模式，切换条件解释为“差异只出现在预期类别（`transient`），`permanent`/`unknown` 无差异”；请确认该解释是否适用于此类工作项 | W0.14 | 交付规范 | 已决定（决策 72） |
| 2026-11-03 | Worker control 的 `/v1/health/ready` 只看运行/排空状态，不含数据库迁移与审计失败判定，与 Worker 心跳的就绪口径不一致；是否统一（含 W7.8a 的迁移兼容判定） | W0.14、W7.8a 调研 | W0.14 后续或 W7.9 | 待用户决定 |
| 2026-11-03 | W0.9 的 `/metrics` 转发头集合补 Cloudflare 头（`cf-connecting-ip`、`cf-ray`、`true-client-ip`）：W0.12 确认站点在 Cloudflare 后面 | W0.12 | W0.9 后续或 W9.5 | 已随 W9.5 实现（`b7b2619a`，2026-11-08 源码审计核实） |
| 2026-11-06 | W2.6 的 COD 三次退回终态已获用户批准新增 `cod_returned`；迁移 `000100` 扩展 `delivery_dispositions` 的来源列与 kind 闭集，处置绑定原 COD 投递并引用恢复记录。 | W2.6 审计 | W2.1/W2.6 | 已决定（本次实现） |
| 2026-11-08 | 被超提醒的论坛 PM 循环不受 `external_access_paused` 约束，暂停期间照常发送与核对；W10.9 恢复核对依赖全局暂停 | 源码审计 | W1.9 | 已修复（`bb1195d0`） |
| 2026-11-08 | 结算命令重放读取已结算事实时对只追加表 `auction_settlements`、`lot_settlements` 加 `FOR UPDATE`，运行账号没有 `UPDATE` 权限而报 `42501`；W9.9 部署后重放路径必然失败 | 源码审计（运行账号 PG18 用例） | W9.9/W6.2 | 已修复（随本提交，改为普通读取；场次行锁与只追加触发器已提供保护） |
| 2026-11-08 | 人工 Sent 预览/确认对非结算投递（载荷形状不同）或从未进入发送尝试的投递以反序列化错误中止，而不是返回 `unsupported_delivery_scope` 等闭集阻断项 | 源码审计（运行账号 PG18 用例） | W3.1 | 已修复（随本提交，载荷宽松投影为 `None`，缺失 fence 投影为 0） |
| 2026-11-08 | 人工 Sent 证明的准入成功分支没有 PG18 夹具（需完整发送尝试、凭据与资金事实）；其余 PG18 用例仍以维护账号驱动被测端口，可能掩盖运行账号缺权；存储端口覆盖率无统计工具 | 源码审计 | W6.2 | 待排期 |
| 2026-11-08 | W0.14 与 W1.4 的影子模式没有复用 W0.15 共享框架（`auction_shadow_*` 无样本），按交付规范第 3 条“无样本的分支不算通过”无法据共享指标判定切换；需改用各自计数器作为切换依据，或迁移到共享框架 | 源码审计 | W0.15 | 待用户决定 |
| 2026-11-08 | 原则 5 未普遍落实：运行时开关闭集只有 7 项，W1.1/W1.3/W1.5/W1.7、W3 各批、W12.1 按场次排队、W9.8、W14.4–W14.6/W14.11 等行为变更没有运行时开关与旧路径，W3 也未做影子比对 | 源码审计 | 原则 5 / W3 | 已决定（决策 83）：不补开关，风险写入部署清单 |
| 2026-11-08 | CLI `funds` 的身份与现金证据仍直接建立 MoogleMail/论坛客户端（要求先停 Worker 并取得凭据 owner，不与 Worker 争用），未经 Worker control | 源码审计 | W1.10 | 待用户决定（建议保持） |
| 2026-11-03 | 线上 `19344` 端口由 sing-box 监听在所有网卡，与拍卖服务无关但扩大暴露面 | W0.12 | W9.6 | 待用户决定 |

## 7. 进度

状态取值：未开始、进行中、已提交、已部署、已验收、暂缓、取消。工作流行汇总其工作项；开始某个工作项时在表中另起一行（如 `W1.0`），备注写执行者与分支，完成后填写提交号，部署与验收结论由用户部署后补记。

| 编号 | 状态 | 提交 | 部署 | 备注 |
| --- | --- | --- | --- | --- |
| D1 | 待诊断 | — | — | W0.7 诊断已提交（`0f36d1e8`），待部署后按日志 `safe_kind`/`stage` 与只读核对确认根因；代码阅读的首要嫌疑为原投递 `sending_at` 早于资金初始化/对账边界，另需解释“未持久化错误”的第二层原因 |
| D2 | 已提交 | `b2ccb9fd` | 未部署 | W1.0；线上 Worker/CLI 为 `f99a9d4e` 构建、Public/Admin 仍为 `92259475` 构建，均未包含 W1.0 修复 |
| D3 | 已提交 | `fa46d605` | 未部署 | W0.12 估计 6–9 周触及 100 页上限；决策 74 提前交付 W1.4 发送基线；默认 `shadow`，须在触顶前观察并由用户切换 `watermark` |
| D4 | 已决定 | — | — | 决策 14；W1.3、W1.7 随 M2b |
| D5 | 待量化 | 量化指标 `5ebfc1c8`；W12.1 已提交 | 未部署 | W0.6 的 `auction_db_transaction_*{owner,phase}` 部署并经 W0.10 采集后，以 Final 阶段 `bid`/`proxy`/`slow_proxy` 的耗尽次数决定是否提前部署 W12.1；W12.1 本身已实现，不是待实现项 |
| D6 | 已决定 | 说明 `6cc3769f`；五语言稿 `383138a8` | 未部署 | 决策 15；`docs/guide` 玩家说明已提交（Claude 依代码核实：回帖无发帖时间，`PlaceBidCommand` 按导入时服务端时钟判定）；五语言编辑稿在 `ops/rules/forum-reply-import/`，已有规则须在 Admin 编辑发布，`rules import` 仅首次/缺失语言；W0.12 显示主题最多约 27 楼，远低于页数上限，W1.11 不提前 |
| D7 | 已提交 | `4ba5abe3` | 未部署 | 决策 32、71、72；W0.14 默认影子模式，切换 `classified` 后生效 |
| D8 | 已提交 | `4ba5abe3` | 未部署 | 决策 33；W7.11 |
| D9 | 已决定 | — | — | 决策 62；W14.6 随 M2b |
| D10 | 已决定 | — | — | 决策 60；W14.4 随 M2b，W0.12 发现滥用则提前单独交付 |
| D11 | 已提交 | W14.5 `a7bc4ef3` | 未部署 | W14.5 已实现 PM 渠道不可用返回 422 并提示；剩余为 M2b 部署后的运行时验收 |
| 计划外 | 已提交 | `7ba074a5`、`f99a9d4e` | 已部署 Worker/CLI | 履约证据预算与内部证据快照时效修复；2026-11-02 两次停机部署，Worker/CLI 为 `f99a9d4e` 构建，Public/Admin 未变 |
| W0 | 已提交 | — | 未部署 | W0.1–W0.16 已提交；W0.6 服务超时默认关闭、W0.10 指标出口与 W0.14 `classified` 待用户启用并观察；W0.15 共享影子框架只接入了 W2.5 处置读侧（Worker/Admin），W0.14 与 W1.4 的影子模式各用自有计数器（2026-11-08 源码审计） |
| W0.1 | 已提交 | `0f36d1e8` | 未部署 | 执行者：Codex（gpt-6.1-sol）实现、Claude 监督；分支 `refactor/dioxus-frontend`；`ApplicationError::safe_kind()` 闭集分类，首个调用方为履约 reconcile 未持久化错误日志 |
| W0.7 | 已提交 | `0f36d1e8` | 未部署 | 执行者：Codex（gpt-6.1-sol）实现、Claude 监督；分支 `refactor/dioxus-frontend`；COD 实收资金化各拒绝分支细分为独立 `safe_kind`（target `auction::settlement::funding_diagnostics`），持久化 defer reason 不变、无迁移；部署后按 `ops/README.md` 的 D1 诊断章节收集分类证据 |
| W0.14 | 已提交 | `4ba5abe3` | 未部署 | 执行者：Codex（gpt-6.1-sol，车道 A）实现、Claude 监督；审计失败按 SQLSTATE 分类（决策 32、71），`[readiness] audit_failure_mode` 默认 `shadow`、就绪缓存 1 秒；观察到差异只在 `transient` 类后由用户显式切换 `classified`（决策 72），W0.6 的超时须在切换后启用；2026-11-06 审计按用户决定恢复 shadow 默认 |
| W0.12 | 已提交 | 主机与数据库 `5e2f455b`；浏览器 `fddd9fbb` | — | 执行者：Claude（经用户批准的线上只读核实）；结论见[docs/plans/architecture-improvement/baseline.md#w012-线上只读核实2026-11-03](architecture-improvement/baseline.md#w012-线上只读核实2026-11-03)；main Sent 37 页 549 行、MID 跨分钟严格递减（同分钟可逆序），D3 约 41 天触顶；论坛发帖时间到分钟、按账号时区（UTC+8）显示；isekai 邮件因角色战斗中未读，留待下次核实 |
| W0.9 | 已提交 | `57d1748d` | 未部署 | 执行者：Codex（gpt-6.1-sol，车道 E）实现、Claude 监督；HTTP 请求量/状态码/延迟、WebSocket 连接、限流拒绝指标；`[metrics] reject_forwarded_requests` 默认 true（带转发头的 `/metrics` 请求返回 404），CDN 头待 W0.12 |
| W0.11 | 已提交 | `0a30c5c8` | 未部署 | 同上；`RateLimitedLog` 固定类别 60 秒窗口去重，首个调用点为 delivery 循环失败日志；`log_filter` 加 `auction_observability::log_dedup=off` 可关闭 |
| W0.2 | 已提交 | I-A `46eb1c43`；I-B `adbc438f` | 未部署 | 执行者：Codex（gpt-6.1-sol，车道 I）实现、Claude 监督；I-A 覆盖 jobs/application 152 个位点，I-B 覆盖 integrations 与 HTTP 477 处记录入口（74 个 owner kind、532 组 kind/phase，清单见 `ops/README.md`），`SafeErrorKind` 共 113 类；持久化 defer reason/失败码、HTTP 状态码/错误码/JSON 不变（用户决定）；Public/Admin 随原服务生命周期安装日志去重门；棘轮 HTTP `Err(_)` 118→73、`map_err` 121→94，integrations 24→1、363→64 |
| W0.3 | 已提交 | `46eb1c43` | 未部署 | 同上；`auction_worker_unpersisted_error_total{loop,kind}`，口径为未确认形成持久失败、退避或人工处置（提交结果未知计入），先于日志去重累加；PM 暂归 `forum` loop |
| W0.10 | 已提交 | `421ee64c` | 未部署 | 执行者：Codex（gpt-6.1-sol，车道 L）实现、Claude 监督；`common.toml [metrics]` 三角色可选 loopback 出口（只允许 `127.0.0.1`，示例 19100/19101/19102，省略即关闭），复用 W0.9 `/metrics` 契约，出口故障只降级采集；`ops/monitoring/` 提供 Prometheus、exporter 查询与 unit 示例；待用户按 `ops/README.md` 安装 Prometheus、node_exporter、postgres_exporter 并启用出口后验收；W0.13 随后续批次 |
| W13.3a | 已提交 | `7a532130` | 未部署 | 执行者：Codex（gpt-6.1-sol，车道 N）实现、Claude 监督；release 开启 `overflow-checks`（`wasm-release` 继承，保留 abort），原生装配根与 CLI 安装安全 panic hook（target `auction::panic`，只输出线程、位置与 `&'static str` payload），顶层 JoinError 固定分类；Worker 已有子任务收口新增 `auction_worker_task_panics_total{loop}`，处置与计数不变；逐 owner panic 计次拆为 W13.3b 待用户决定 |
| W0.16 | 已提交 | `386cc963` | — | 匿名浏览器错误信标：闭集 class/build、限流、空 body、只累加指标；WASM panic/DTO/实时协议接入 |
| W0.15 | 已提交 | `84e0fba5` | — | 共享影子运行端口、`auction_shadow_*` 指标与结构化日志；闭集 owner/branch、零初始化、不可比较样本不伪造覆盖。2026-11-08 源码审计：生产调用点只有 W2.5 处置读侧；`audit_readiness` owner 只用于测试演示，W0.14 与 W1.4 未复用本框架 |
| W13.3b | 已提交 | `b7b2619a` | — | panic 指标按 `loop,owner` 闭集；生命周期/结算首 panic 写 fenced terminal，履约/recovery 走 manual_reconcile，delivery 按 claimed/sending 区分；slow proxy/关键循环失败关闭；无迁移/无新终态 CLI |
| W0.6 | 已提交 | `5ebfc1c8` | 未部署 | 执行者：Codex（gpt-6.1-sol，车道 K）实现、Claude 监督；`[database_profiles] mode` 默认 `profiled`（`legacy` 为旧路径），服务池 16、维护池 4、固定 `application_name`，`deployment check` 汇总连接预算；服务超时默认关闭，启用须显式填写阈值且以 W0.14 切换 `classified` 为前置（见待决定）；21 个事务 owner 的尝试/冲突/重试/耗尽指标，Worker 循环连接获取与持有指标；切换需重启 |
| W0.4 | 已提交 | `bc0e5430` | 未部署 | 执行者：Codex（DeepSeek V4.1 Flash，车道 M）实现、Claude 监督；Worker 装配同步外部指标记录器，覆盖 19 个邮件操作的物理请求、请求门/独占锁/incoming 读写锁等待、`account_busy`/限流/超时信号与维护/观察/fetch/批次/扫描预算耗尽；关闭 metrics 或非 Worker 为 no-op，不改变请求间隔、锁顺序、deadline、重试、取消、写入围栏或结果未知处理 |
| W0.5 | 已提交 | `bc0e5430` | 未部署 | 执行者：Codex（DeepSeek V4.1 Flash，车道 M）实现、Claude 监督；论坛物理请求、门等待、主题扫描发现/处理页数、写入未知、PM prepare/send/reconcile 与回帖导入延迟 `poll_window` 上下界估计（终点为每帖首条新命令账本提交确认，不填零）；`ForumCommandStart.is_new` 与 `ForumReplyTopic.reply_phase` 是只读投影，不参与命令裁决；论坛 world 允许 `shared` 仅限无世界资料读取 |
| W0.13 | 已合入 main | `5b87ae11`、`9df059c9`；索引迁移 `d0de52a5` | 未部署 | 执行者：Codex（gpt-6.1-sol，车道 L）实现、Claude 监督；Worker 每 60 秒只读汇总窗口 Gauge（只借空闲连接、Repeatable Read 只读、2 秒预算，候选超 10000 整轮不可评估），`[metrics] sli_snapshot_enabled` 默认关闭、W7.7 开关 `metrics_sli_snapshot_enabled`；已实现实时 relay/WS 写出延迟、Final 出价容量、慢速代理当前积压；出价延迟与冲突引用 W0.9/W0.6，Final 回帖导入引用 W0.5；迁移 `000077` 已补五个全局时间候选索引（决策 77）；八项派生窗口已实现：注册 key、提醒余量、结算首/末封、买家至卖方、投稿 Take/拒收、慢速代理历史；缺强证明候选统一保留并计 unknown，结算混合证明场次不发布首/末封分位。按终点落窗、候选 10001 哨兵；坏 metadata 整轮失败，付款证明缺失计 unknown；未完成覆盖暂不可用（incomplete_available=0），不填零。记录规则与告警模板在 `ops/monitoring/`，SLO 目标值待基线观察后提出 |
| W0.8 | 已提交 | `e78dac4f` | 未部署 | 执行者：Codex（gpt-6.1-sol，车道 D）实现、Claude 监督；`storage_error_from`/`sql_error_context` 附加 sqlx 变体、SQLSTATE、约束名、表名；1100 处直接 SQLx 闭包与 13 个具名 mapper 一次改完，`storage:map_err(\|_\|)` 1432 → 330；其余 330 处另定范围 |
| W7.11 | 已提交 | `4ba5abe3` | 未部署 | 执行者：Codex（gpt-6.1-sol，车道 A）实现、Claude 监督；围栏调用前登记关键段，`[shutdown] critical_write_timeout_seconds` Worker 默认 60（`0` 为旧路径），Worker unit 120s、server 225s；行为验收待 W6.6 替身 |
| W7.8a | 已提交 | `3f0a2097` | 未部署 | 执行者：Codex（gpt-6.1-sol，车道 F）实现、Claude 监督；迁移 075 建只追加兼容登记表（001–074 缺行即 breaking，075 自身 breaking），`check.sh migration-compatibility` 检查每个新迁移恰好登记一次；`[readiness] migration_compatibility_mode` 默认 `shadow`，回退旧程序前须设为 `compatible`；本版含 breaking 迁移，部署后才能为之后的版本提供回退能力 |
| W7.7 | 已提交 | 第一批 `5e2f455b`；第二批 `dff019c4` | 未部署 | 执行者：Codex（第一批 gpt-6.1-sol，车道 H；第二批 DeepSeek V4.1 Flash，车道 O）实现、Claude 监督；迁移 076 只追加开关事实兼命令账本、三角色 5 秒刷新、`auction-cli switch`、Admin `/config` 只读卡片；闭集共 6 项：`readiness_audit_failure_mode`、`readiness_cache_enabled`、`metrics_sli_snapshot_enabled`（W0.13）、`mooglemail_poll_isolation_enabled`（每世界常驻维护任务 + 互斥门，在途轮次按开始时模式结束，不重写 `next_poll_at`）、`metrics_reject_forwarded_requests`（只允许在线收紧，`set false` 以 `UnsafeRelaxation` 拒绝）、`external_access_paused`（W1.9）；W1.9 主动暂停随 M2b 复用 |
| W1.11 | 已提交 | `822fb781` | — | 论坛导入游标来自命令账本最高 post_number/post_id；固定楼层页+游标页增量，删帖/楼层前移回退全量；Final 按最早 lot 截止收紧轮询；无迁移 |
| W7.10 | 已提交 | `822fb781` | — | Worker 热读取发送节奏、邮件/论坛预算、超时/批量与 W1 预算/退避；完整校验后原子替换，失败保留旧值并告警；Admin 标注热字段 |
| W9.10 | 已提交 | `822fb781` | — | `rekey` 主题轮换登录 key、注销全部会话、24h 账号回信预算；五语言模板/配置 |
| W9.11 | 已提交 | `822fb781` | — | 说明信 10/24h 上限，超额仍退回资产但不寄说明；`auction_mooglemail_notice_suppressed_total` |
| W1.1 | 已提交 | `8fa488f1` | — | 进程内 per-world 账号忙熔断 1→2→5→10 分钟；窗口内仅一次读探测，写请求在 fence 前拒绝；证据每忙碌期保留一次 |
| W1.2 | 已提交 | `8fa488f1` | — | 忙碌诊断每期去重；淘汰优先保留 write_unknown，普通证据按 mtime；不可读头 fail-closed |
| W7.6 | 已提交 | `c9e5e3b5` | — | 响应头/WS 首帧构建号；浏览器旧 bundle 仅提示、用户点击刷新；构建号随 Web payload 输入 |
| W14.3 | 已提交 | `f86f1c08`、`c9e5e3b5` | — | 投稿接收回执、卖方每场汇总、isekai 换季领取/交付提醒；sending 围栏前失效；迁移 082 breaking |
| W7.9 | 已提交 | `8fa488f1` | — | Public/Admin 对 Worker/Admin 改为 `Wants=`，保留 `After=` 启动顺序；systemd 模板静态门禁；合并部署仍单故障域 |
| W9.8 | 已提交 | `8fa488f1` | — | Public 会话上限默认 10；用户行锁内注销最旧活动会话，不删除记录/不改登录 key；五语言“其他设备登录”提示；PG18 并发测试 |
| W1.3 | 已提交 | `a7bc4ef3` | — | 优先级调度（告警>账号>按截止出价>背景）、1 容量无突发门、独占操作锁、incoming 分段释放；2026-11-06 审计修复迁移 098：082 漏回 `rekey_key`/`rekey_result` 的 `account` 分类并重建表达式索引 |
| W14.2 | 已提交 | `a7bc4ef3`、`0448a921` | — | 出价被超含 lot/装备/金额/截止/链接；结算 COD/carrier/payout/refund 增加付款期限与装备名，历史冻结形状裁剪保持 |
| W14.4 | 已提交 | `a7bc4ef3` | — | 论坛 notify 改为订阅（帖内 8/账号每场 64），不再回执 MM |
| W14.5 | 已提交 | `a7bc4ef3` | — | 网站普通出价自动标记；PM 渠道不可用返回 422 并提示 |
| W14.6 | 已提交 | `a7bc4ef3` | — | 付款提醒需冻结付款发送证明，否则延后并审计；2026-11-06 审计补齐 `recovery_payment_mail_unsent` 告警来源（迁移 099，覆盖 awaiting_payment/payment_overdue 的未证明付款邮件），W5.5 页数/换季来源已提交（`cdce3df8`），尚未部署 |
| W14.11 | 已提交 | `a7bc4ef3` | — | 慢代理可行动期仅记 pending，耗尽后按行动结果发提醒；截止清理。2026-11-06 审计修复：慢代理计划及普通出价在到期/Final 反超成功或耗尽时于同一事务解析 pending；Final 不再被错误当作 lot 关闭。补充修复 Final 代理审计闭集接受 `exhausted/strictly_defeated`，pending 的 MM 收件人按迁移约定冻结用户名，并新增 PG18 回归覆盖 Final 耗尽后 pending 入队及改名场景 |
| W1.6 | 已提交 | `0448a921` | — | 论坛客户端复用 PriorityGate，写流程前台、读后台；保留论坛自身 429/未知写入分类 |
| W14.9 | 已提交 | `0448a921` | — | 零资产收件人不存在失败关闭；发送前用户名改名退役 lot_watch 提醒 |
| W1.4 | 已提交 | `f86f1c08`、`c7335af4`、`bb071773` | 未部署 | Read Mail 与 Sent 前缀扫描/每日完整校验/回退；迁移 083 持久化扫描水位，按 credential/version/world/endpoint 隔离；Inbox DB 游标权威。Sent 常规维护命中水位锚点时只读前缀，锚点缺失、顺序异常或超限回退完整枚举；完整枚举页数超限只告警并推进每日校验时间，不阻断发送。 |
| W1.5 | 已提交 | `386cc963` | — | 履约证据按 retry_count 1→2→4→8→10 分钟退避，付款截止前后回到基础频率；不改期限/策略 |
| W1.7 | 已提交 | `651b1191` | — | 出价被超提醒在 lot 有效截止后、sending 围栏前退役；MM/PM 一致；五语言稿 |
| W1.9 | 已提交 | `651b1191`；PM 补漏 `bb1195d0` | 未部署 | `external_access_paused` 运行时开关与到期回退；Worker 各外部轮次 admission 门；已越过 fence 的写入不取消。2026-11-08 源码审计发现被超提醒的论坛 PM 循环（`LotWatchPmRuntime`）未持有开关、暂停期间照常发送与核对，已补暂停门与 tokio 暂停时间回归测试 |
| W1.10 | 已提交 | `651b1191` | — | Admin 装备读取与 CLI Sent 核查经 Worker control；共享调度/熔断；Worker 不可用明确能力不可用 |
| W14.1 | 已提交 | `651b1191` | — | 22 类通知目录（触发、收件人、渠道、出队、失效、模板键）与模板注册一致性测试；出价被超 MM/PM 已按 `lot_watch`/`lot_watch_pm` 模板在创建时渲染并冻结，论坛 notify 已改为订阅、不再回执 |
| W14.8 | 已提交 | `651b1191` | — | 通知 created/retired/suppressed/sent/failed 指标、失败原因、延迟与邮资估算；PM/MM 接入；非结算告警待迁移 |
| W1 | 已提交 | — | 未部署 | W1.0–W1.11 已提交；发送基线、告警优先级、论坛增量与外部访问暂停均落地 |
| W1.0 | 已提交 | `b2ccb9fd` | 未部署 | 执行者：Codex（gpt-6.1-sol）实现、Claude 监督；分支 `refactor/dioxus-frontend`；完整 Read Mail 观察降频为每小时、维护独立循环、isekai 相位错峰，回退开关 `poll_isolation_enabled`；待用户部署后按“维护不再超时、`mail-maintenance-*` p95 < 60 秒”验收 |
| W1.4（发送基线切片） | 已提交 | `fa46d605` | 未部署 | 执行者：Codex（gpt-6.1-sol，车道 J）实现、Claude 监督；发送前临时 MID 边界 + 前缀扫描，`[worker.mooglemail] sent_baseline_mode` 默认 `shadow`；切换 `watermark` 前须完成浏览器 MID 语义核实与至少一个完整场次周期的影子观察；常规轮水位与每日校验已随本批次补齐 |
| W2 | 已提交 | `ed7031be` | — | 迁移 088 处置事实与确认命令；Admin 判失败/装备损失/confirm-sent/预检退役/告警收口。2026-11-06 审计补齐“确认已处置”Admin 详情按钮与 HTTP 路由，并在详情中回显统一处置事实；同时修复 `equipment_count` 的 INT4/INT8 解码错误。W2.5 增加 `disposition_read_mode`（legacy/shadow/classified，默认 shadow、旧口径裁决）与同语句配对影子样本；W2.6 装备损失线下补偿与 COD 三次退回均由统一处置事实解除（迁移 100；COD 使用 `cod_returned` + `settlement_recovery_id`；真实 `advance_due_cod_return` 回归验证同事务三事实与重试幂等） |
| W3 | 已提交 | `a1c85784`、`5e43f90a` | — | W3.1–W3.8：准入/状态机/排期/Admin 守卫/COD 收据/claim 一致性；无迁移收窄 |
| W4 | 已提交 | `a1c85784`、`4319e60f`、`ed7031be` | — | W4.1–W4.6 详情/检索/玩家支持已完成；迁移 087。2026-11-06 审计补齐确认已处置入口，并让详情读取统一处置表的全部种类（自动 Sent 证明、装备损失、历史自寄、预检退役等）；W4.1 收口 |
| W5 | 已提交 | `a9350b9d`、`6f161bdc`、`cdce3df8`、`679e3cac` | 未部署 | 089 告警台；091 主机侧备份/磁盘/WAL；CLI `alert report`；readiness 默认 shadow（classified 待观察后由用户切换）。2026-11-06 审计拆分 `scan_enabled`（告警台扫描，默认开）与 `notify_enabled`（MoogleMail 通知，默认关）；付款邮件未寄出告警随 W14.6 通过迁移 099 补齐。W5.1 独立 webhook 通道已提交但默认关闭（迁移 102；尝试 5 次后转 dead-letter，Dashboard 计数与只读投递视图 `GET /api/v1/admin/alerts/webhooks`）；W5.2 仪表盘与扫描器共用阈值/等待期判定；W5.3 外部账号只读快照（Worker control `/v1/external-accounts`）；W5.4 活动与恢复摘要锚点及 Admin 可点击链接已提交；W5.5/W5.6 已提交，尚未部署；W5.7 处置手册与 W5.8 告警台已落地。外部 Webhook 与心跳打卡仍待用户选定服务 |
| W5.5 | 已提交 | `f834fd4b` | 未部署 | Sent/论坛远端页数 80% 与冻结换季边界前 isekai 未完成义务：迁移 `000101` 与源码已提交，PG18 回归覆盖页数作用域、有效上限、过期观察、论坛可扫描场次与独立观察时钟、换季 contingent/manual 口径；尚未部署 |
| W5.6 | 已提交 | `f834fd4b` | 未部署 | Worker 外部存活心跳：只在 readiness 正常时每 60 秒向敏感配置的 HTTPS 地址发送受限 GET；地址默认空即关闭，启用仍受决策 50 约束；尚未部署/启用 |
| W6.3 | 已提交 | `2241689b`、本分支 | 未部署 | MoogleMail、公开论坛主题/登录、PM 列表/详情、主世界战斗页均有真实脱敏样本；论坛与邮件 manifest 分开维护 |
| W6.1 | 已提交 | `5a06c26d` | — | 56 个纯逻辑回归：结算守恒/持有曲线/冲正、竞价 v1/v2/代理/软截止、排期 DST/早鸟、邮件/论坛解析、前端实时与查询；未扩大私有 API |
| W6.2 | 已提交 | `84e0fba5`；结算/恢复/人工 Sent 与运行账号用例随本提交 | — | PostgreSQL 18 测试骨架：临时集群/模板库/每测试克隆与清理；CI 维护角色权限已修正。2026-11-08 补以运行账号（`auction_runtime` 成员）执行的结算端到端、付款邮件未证实时的恢复延后与人工 Sent 拒绝分支，并据此修复结算重放 `42501` 与 Sent 预览反序列化中止；共约 85 个用例，人工 Sent 成功分支与 157 端口逐方法覆盖仍待扩展 |
| W6.5 | 已提交 | `0ff6bfcc` | — | `just test` 运行默认 feature 全 targets；`just test-db` 要求隔离 PG18 的 `AUCTION_TEST_DATABASE_URL`；默认 verify 仍不运行测试；AGENTS/CONTRIBUTING/开发指南与计划交付规范同步 |
| W6.6 | 已提交 | `0ff6bfcc`、`2be66549`、`c4058b20` | — | 2026-11-06 真实 MM 测试发送成功页已脱敏入库；覆盖发送成功证明；内部证明/偿付聚合回归已补 |
| W6.4 | 已提交 | `26af8ea7` | — | CI 独立 `test` job：安装 PostgreSQL 18、运行 `just test` 与 `just test-db`；不并入默认 verify。2026-11-06 审计修复维护角色权限：CI 的 `auction_test` 显式授予 `CREATEDB`/`CREATEROLE`，服务运行账号不授予 |
| W6 | 已提交 | — | — | W6.1–W6.6：真实脱敏样本、loopback 替身、写表单成功页、PG18 与 CI 测试 |
| W7 | 已提交 | `52a113fb` | — | W7.1 发布流水线、W7.2 deploy/rollback、W7.3 回执、W7.8 兼容验证；W8.1 合并部署决定前仅支持分体部署 |
| W7.1a | 已提交 | 远端设置 | — | 仓库转公开；`main` 已设为默认分支（当前 `383138a8`），分支保护要求 `docs`/`verify`/`security-audit`/`msrv`、线性历史、禁止强推与删除；`ci/dependency-version-guard` 与两个 dependabot 分支已删除，tokio PR #10 已按决策 52 关闭；`main` CI 成功，候选二进制为 `24a9c115` |
| W7.2 | 已提交 | `52a113fb` | 未部署 | `auction-cli deploy/rollback` 已实现制品校验、时段与在途发送门禁、迁移演练回执、停机备份与新旧 CLI 校验、保留旧程序、原子安装、readiness 与回执；2026-11-06 审计补齐回执 `migration_drill_id`，演练回执不再接受 `not_available` |
| W7.5 | 已提交 | `69fd85a0` | 未部署 | 执行者：Codex（gpt-6.1-sol，车道 C）实现、Claude 监督；`service stop/restart` 只读时段门禁（Final 或截止不晚于保护窗口末端的在拍 lot，含已过截止仍在拍），`--protection-window-minutes` 默认 60，例外须 `--force --reason` |
| W8 | 已提交 | — | — | 分体部署；保留 setup/isekai；Redis 移除；realtime_consumer 与四 schema 对象删除（097）；全仓 #[path]=0；application 按 23 个上下文分组；后续拆分 admin api、mail maintenance、MM sender、delivery runtime |
| W9.7 | 已提交 | `2367edf6` | 未部署 | 执行者：Claude（用户批准 M2a）；CLI 审计 `source=cli`，metadata 添加 `entrypoint=cli`、`os_uid`、可选 `sudo_user`；`--actor` 仍必填；Admin 保持 `source=admin`；迁移 078 breaking；Admin 审计页支持 cli |
| W9.9 | 已提交 | `c9e5e3b5` | 未部署 | 迁移 081/097 的运行账号授权与 `config check --strict` 拒绝 owner；2026-11-06 审计修复合并部署首次设置：`auction-server --owner-secrets` 用 owner 初始化空库，服务账号不执行 DDL，并校验 owner 与服务指向同一 SQLx 解析目标 |
| W9.5 | 已提交 | `b7b2619a` | — | Admin 全局登录退避（5 次后 1→60s）、IPv6 /64、失败 WARN、复用 401/429 指标；首次设置 64 hex 随机密钥且最短 32 字节；短密钥/可信代理警告；拒绝 /0 trusted proxy；CDN 专用转发头拒绝 |
| W9 | 已提交 | `9daf57be`、`5e43f90a`、`84051a2e` | — | W9.3 密钥环/离线重加密；W9.4 HSTS/form-action；W9.5–W9.11 已落；W9.11 的账号/说明信预算已纳入 Worker 热读取；无部署 |
| W10 | 已提交 | — | 未部署 | W10.1–W10.9 已提交；W10.4 purge 已隔离执行，生产删除未执行 |
| W10.1 | 已提交 | `88dea2a6` | 未部署 | 执行者：Codex（gpt-6.1-sol）实现、Claude 监督；分支 `refactor/dioxus-frontend`；`backup scheduled` + systemd timer 模板（每日 UTC 03:15），回执 `auction-backup-run.v1`；本地 2026-11-04 已启用用户级 timer 并完成一次 systemd oneshot create+verify+retention；仍待连续 7 天成功与一次人为失败核验 |
| W10.2 | 已提交 | `88dea2a6` | 未部署 | 执行者：Codex（gpt-6.1-sol）实现、Claude 监督；分支 `refactor/dioxus-frontend`；主机 `age` 公钥加密 + `object_store` 上传 R2 日/月前缀、完整回读校验，`[backup.offsite]` 默认关闭；本地 2026-11-04 已用 `hv-auction` bucket 完成首次 daily/monthly 上传、bucket lock 覆盖/删除 409、age 解密与隔离 PG18 恢复；仍待三组数据密钥加密列解密、生命周期规则核验与 `age` 私钥离线托管 |
| W10.6 | 已提交 | `69fd85a0`；演练 2026-11-04 | 未部署 | 执行者：Codex（gpt-6.1-sol，车道 C）实现、Claude 监督；手工流程 `ops/deploy/MIGRATION-DRILL.md` 与回执模板 `auction-migration-drill.v1`。本地隔离 PG18 演练：schema 74→77 applied=3，旧/新三角色 readiness 200，停机实测 46 秒、含备份/安装/单元的估算 60 秒；回执 `target/operations-evidence/m1-drill-20261104T053636Z/migration-drill-receipt.json`，结论 incomplete（W11.4 not_available、评审待签字）。2026-11-06 审计收紧：W11.4 已落地，演练回执必须 `passed` 并附证据，`not_available` 不再放行；旧回执须重新演练 |
| W10.7 | 已提交 | `5a06c26d`、`c3c1aaf8`、`6cb1944a` | 未部署 | WAL archive/upload、zstd+age、R2 `wal/`、weekly `pg_basebackup` 至 `base/`、保留 3 份、systemd timer；本地隔离 PG18 PITR 演练通过：恢复到目标时间 rows=1,2，排除之后 rows=3 |
| W10.8 | 已提交 | `887c8b6d` | 未部署 | 执行者：Claude（用户批准 M2a）；`ops/postgres/postgresql.conf.example`、`autovacuum.sql.example`、`extensions.sql.example` 与说明；连接预算 64/97、`pg_stat_statements`、慢查询/锁等待/autovacuum 日志、热行表与只追加表参数；`archive_mode=off`，W10.7 再启用；重启受 W7.5 |
| W11.4 | 已提交 | `b7b2619a` | — | 只读一致性巡检：lot/bid 投影、补偿池、义务链、delivery/义务、realtime 序号、慢代理残留；每项预算 5s/总 40s，最多 100 样本；结果追加 audit_records；CLI `db inspect-consistency` |
| W11 | 已提交 | `5e43f90a`、`9daf57be`、`b7b2619a` | — | W11.1–W11.4：装备持有核对按 expected/stock/personal 分类，偿付能力、追加报告、一致性巡检；迁移 095 |
| W12 | 已提交 | — | 未部署 | W12.1–W12.8 已提交：outbox、队列、提交后唤醒、键列化、体积预算。W12.1 已实现，D5 只决定生产指标确认后是否提前部署，不是待实现 |
| W13 | 已提交 | — | — | W13.1–W13.6：棘轮、lint、溢出防护、链接门禁、依赖审计与季度升级窗口 |
| W13.1 | 已提交 | `42bd8477` | 未部署 | 执行者：Codex（gpt-6.1-sol，车道 B）实现、Claude 监督；`check.sh ratchet`，基线 `ci/checks/source-ratchet.baseline`（14 项） |
| W13.4 | 已提交 | `42bd8477` | 未部署 | 同上；`check.sh markdown-links` 离线检查相对链接与锚点，现有文档无断链 |
| W13.5 | 已提交 | `42bd8477` | 未部署 | 同上；CI 每周一 04:17 UTC 运行 `security-audit`；发布 tag 审计待 W7.1 |
| W13.2 | 已提交 | `0a30c5c8` | 未部署 | 执行者：Codex（gpt-6.1-sol，车道 G）实现、Claude 监督；workspace `unwrap_used`/`expect_used = deny`，测试上下文经 `clippy.toml` 豁免；宏展开内的调用不受 Clippy 覆盖，见 `ci/checks/README.md` |
| W14 | 已提交 | — | — | W14.1–W14.6、W14.8–W14.11 已提交；W14.7 取消。2026-11-08 源码审计补齐 W14.10 玩家指南“会收到的邮件与提醒”、`rekey`、自动标记（含论坛回帖）、PM 绑定前置与慢速代理待定提醒，并更正业务规则中的出队优先级（随本提交） |

### M1 收尾记录（2026-11-04）

- 代码与本地验证：`main` 已含 M1 工作项，本地 schema 77；R2 首次 daily/monthly 上传、bucket lock 409、age 解密、隔离恢复已完成；W10.1 timer 已启用并完成一次 systemd 备份。
- 仍未完成：线上部署与运行时验收（W1.0、W0.4/W0.5、W0.14、W7.5/W7.11、W0.13 生产基线）；W10.2 三组数据密钥加密列解密、生命周期规则核验、`age` 私钥离线托管；W10.1 连续 7 天与一次人为失败；W10.6 回执仍为 `incomplete`（W11.4 属 M2a）及评审签字；SLO 目标值；D6 五语言规则在 Admin 发布。
- 口径更正：W1.4 属 M3，不是 M1 未完成项；W10.2 不要求等待 30/365 天轮换，只要求规则配置/核验；W0.6 服务超时开关不是 M1 硬性阻塞。
- 下一阶段：M2a 先做 W6.3 脱敏回放语料与 W6.6 外部服务替身；已提交 MoogleMail 脱敏邮件子集（Inbox/Read/Sent/详情/compose），论坛/战斗/登录页仍缺，需在获得授权后采集。

## 8. 术语

| 术语 | 含义 |
| --- | --- |
| D / W / M 编号 | D 为已知缺陷，W 为工作流（W0.1 为其中的工作项），M 为里程碑 |
| sending 围栏 | 外部写入前持久化的发送状态与 fencing 版本；越过后结果未知只能人工核对，不自动重发 |
| 目击（sighting） | 维护在 Sent 列表中观察到的投递引用，用于找回或核对发送结果 |
| legacy 自寄 | 旧实例遗留的发给 bot 自身的投递，需要专门证明后处置 |
| 预检退役 | 发送前预检发现提醒已失去资格，直接退役不发送 |
| 水位 | 已完整扫描到的最高远端邮件 ID；只用于缩小扫描范围，不作为发送或收取证明 |
| 论坛游标 | 已处理到的最大帖子 ID 及其所在页；只用于缩小扫描范围，不作为出价证明 |
| 通知目录 | 全部出站通知的种类、触发事实、收件人、渠道、出队位置、失效条件与模板键清单（W14.1），新增或修改通知时同步 |
| 邮资 | HV 对外发邮件收取的费用（金额与计费方式待 W0.12 核实）；余额不足时远端以 `cannot afford the postage` 拒绝发送 |
| 告警台 | Admin 中列出活动与历史告警、支持确认与静默的页面（W5.8） |
| 独占操作锁 | 同一凭据上 compose、资产动作与内部证据共用的进程内互斥锁 |
| 影子运行 | 新旧路径并行计算、只记录差异，不改变实际结果 |
| 运行时开关 | 持久化在数据库、无需重启即可切换的新旧路径选择；只选路径，不决定业务结果 |
| 迁移演练 | 用最新备份在隔离的 PostgreSQL 18 上预先执行新迁移并核对，作为无预发环境的替代 |
| 兼容登记 | 每个新迁移在自身事务内写入的 `compatible`/`breaking` 等级；旧程序据此判断自己不认识的版本能否就绪 |
| 心跳打卡 | 定期向外部服务报到，超时未报到即由外部服务告警（死人开关）；用于 Worker 存活、备份与 WAL 归档 |
| 关键段排空 | 停机时只等待已越过 sending 围栏的外部写入完成并落库，其他工作立即放弃、由租约重领 |
| 外部服务替身 | 由脱敏回放语料驱动的本地 MoogleMail/论坛模拟服务，只用于测试，绝不连接真实账号 |
| 棘轮门禁 | 只允许计数下降、不允许回涨的静态检查 |

业务流程详见[docs/guide/settlement.md](../guide/settlement.md)与[docs/guide/mail-flows.md](../guide/mail-flows.md)。
