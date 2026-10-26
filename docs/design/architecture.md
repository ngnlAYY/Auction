# 系统设计、架构与模块所有权

[文档导航](docs/README.md) · [技术栈](docs/design/tech-stack.md) · [契约](docs/contracts/README.md) · [验证状态](docs/status.md)

## 本页导航

[owner](#crate-所有权) · [能力矩阵](#部署档位与能力) · [事务](#命令事务与事实) ·
[Worker](#worker-与外部副作用) · [数据保护](#配置与数据保护) · [制品](#浏览器实时与制品) · [运维](#生命周期与运维)

## 系统边界

原生平台仅 Linux x86_64，由五个 `apps/*` 装配根的编译期守卫拥有，正式发布 GNU/glibc，不承诺其他 OS/架构或
musl 档；浏览器以 `wasm32-unknown-unknown` 构建，不把原生限制放入共享领域/契约。Public、Admin、Worker 保留独立
进程，`auctionctl` 承担配置、迁移和运维，可选 [auction-server](apps/auction-server/README.md) 以同一二进制复用三个 app 的
lib 入口自管子进程；仅该合并制品允许同时包含两端资源，Public/Admin router 仍按类型隔离，默认分体发布不变。
PostgreSQL、Redis、反向代理和系统服务是外部组件，实现与验收不依赖外部源码仓库。

装配根实例化适配器并经应用端口注入；业务事实原子提交，命令幂等重放，外部结果须有证据，Worker 从持久状态恢复，
能力缺失明确返回而不以空数据或示例伪装成功。

## Crate 所有权

| owner | 拥有的事实或边界 | 实现说明 |
| --- | --- | --- |
| auction-domain | 金额、标识、竞价计划、代理转换、拍卖状态、结算守恒及统一装备分类/命名词典 | [README](crates/auction-domain/README.md) |
| auction-application | 用例、事务编排、命令幂等与窄端口，论坛标色与公开装备分组的共用判定 | [README](crates/auction-application/README.md) |
| auction-contracts | REST/实时 DTO、错误、分页与版本 | [README](crates/auction-contracts/README.md) |
| auction-storage | PostgreSQL SQL、连接池、锁、账本与追加迁移；公开目录在同一快照中按应用分组判定筛选、统计及分页 | [README](crates/auction-storage/README.md) |
| auction-cache | 可重建数据的 disabled/Redis 适配器 | [README](crates/auction-cache/README.md) |
| auction-integrations | 论坛、邮件、装备来源、凭据加密与外部证明 | [README](crates/auction-integrations/README.md) |
| auction-jobs | 持久调度、重试、游标、租约与本机 Worker control | [README](crates/auction-jobs/README.md) |
| auction-http | 两个 surface 的路由、安全边界、TCP/Unix 绑定、静态响应与 CDN 缓存门禁 | [README](crates/auction-http/README.md) |
| auction-web-public | Public 页面、路由、API 消费、实时恢复与译文；装备分组在类型筛选上方，标签消费服务端结果 | [README](crates/auction-web-public/README.md) |
| auction-web-admin | 管理页面、命令交互、API 消费、论坛只读状态刷新与正文诊断、装备词典 DTO 的候选筛选与原生复选框交互（`config/form/equipment_colors/`）、入站诊断代码的展示说明与译文 | [README](crates/auction-web-admin/README.md) |
| auction-web-shared | 无业务偏向的组件、传输、格式化、语言与设计变量 | [README](crates/auction-web-shared/README.md) |
| auction-config | TOML、默认值、校验、cron 与 runtime 文件适配 | [README](crates/auction-config/README.md) |
| auction-observability | tracing、metrics、健康与心跳安全投影 | [README](crates/auction-observability/README.md) |
| auction-process | 终止信号、有界 drain 与任务守卫 | [README](crates/auction-process/README.md) |
| auction-setup | PostgreSQL 单机档的一次性初始化 API、令牌与显式 HTTPS 代理门禁；默认 loopback，不参与正式 Admin 路由 | [README](crates/auction-setup/README.md) |

`apps/*` 只负责配置、依赖注入、资源绑定、关闭顺序和退出码，`src` 根只留 `main.rs`/`lib.rs`，实现放入具名职责
目录，不得用共享大结构体、私有持久化访问或 `common`/`misc` 绕过 owner。数据库对象、事件和路由的唯一 owner 分别
登记在 [schema](docs/contracts/schema.md)、[events](docs/contracts/events.md)和[routes](docs/contracts/routes.md)。

- 装备词典：`auction-domain::equipment::taxonomy` 唯一拥有分类、种类、材质、部位、品质、词缀关联及当前/历史/有限语法元数据；
  名称结果独立保存种类及仅限 Cloth/Light/Heavy 的部位，Legendary 的前后缀由领域强制存在。
  integrations 拥有页面提取、详情类型、等级状态及来源校验，并将历史完整基础名映射至既有 canonical/快照字段；
  application 独立执行收件策略和标色匹配。
  `auction-http::http::admin::runtime_config::taxonomy` 缓存词典的静态只读 DTO，并为当前文档中已解析的有限语法种类补充候选；
  Admin 新种类候选只使用无部位的武器/法杖/盾牌名称及护甲种类，保留已选历史完整名与其精确匹配，
  不导入领域或集成适配器；该投影不进入 runtime 文档或配置指纹。
  配套解析诊断的数据库 CHECK 由 storage 和追加迁移 `000056` 拥有，不新增表、事件、路由或运行配置字段；
  词典响应扩展须显式查询选择，边界见[运行配置契约](docs/contracts/routes.md#runtime-配置与文件事务)。
- 公开装备筛选：`auction-application::ports::forum::colors` 统一拥有稀有/融合与论坛实际标色判定，
  `ports::auction::queries::lot` 拥有包含式等级上限、无等级显示和投影校验；
  `auction-storage::database::lots::postgres::selection` 拥有同一读取快照内的轻量身份扫描、组合筛选计数及当前页 ID 选择，
  不复制匹配规则到 SQL。HTTP 严格映射可选查询、保持响应字段不变；Web 的 `surface/catalog/filters.rs` 独占分组/等级表单，
  `board/data.rs` 绑定查询快照身份，`api/model.rs` 仅解析浏览器查询及校验返回归属；无新持久化能力或迁移阶段。
- 草稿软截止：`auction-application::draft_soft_close` 拥有修正规则与窄端口，
  `auction-storage::database::draft_soft_close` 独占一致快照检查、Serializable/CAS、原命令账本及追加审计，
  `auctionctl` 只装配命令，不在迁移中批量更正旧场次。
- 身份证明：credentials owner 拥有预期 `player_name`、校验及版本传递，integrations 复用其 ownership，无预期名称
  也核验 HV 认证 UID 与论坛目标资料名，缺证据返回 `identity_unproven`，不建第二 Cookie 客户端。
- 凭据获取：application 的 `ports/admin/credentials/acquisition.rs` 定义 Cookie/论坛账号来源、分域摘要与获取端口；
  integrations 的 `credentials/acquire/` 独占新鲜登录表单、受限传输和论坛/游戏字段映射。
  Admin Web 只提交所选来源，HTTP 保持安全响应；原 credential 存储事务、审计、单 active 和 Worker 重启边界不变，无新增迁移。
- 注册账号提示：config 将同源 runtime 的 `bot_players` 投影为 Public/Worker 共用窄名单；
  Public 装配根注入共享只读 DTO，HTTP 的 `public/registration.rs` 拥有 P29，contracts 拥有字段，
  Public Web 的 `api/identity/registration.rs` 与登录页拥有读取和五语纯文本展示。无新持久对象或迁移；
  不把完整配置、邮件凭据或邮箱可用性声明传给匿名浏览器。
- 邮件正文：domain 拥有冻结费率与价格明细的一致性校验，application 拥有通知身份与冻结结算 intent；storage 的
  [claims/pricing.rs](crates/auction-storage/src/database/fulfillment_scheduler/claims/pricing.rs) 投影不可变结算明细；integrations 的
  [settlement/pricing.rs](crates/auction-integrations/src/clients/mooglemail/transport/payload/settlement/pricing.rs) 仅校验和格式化价格解释，
  [outcomes/rejection.rs](crates/auction-integrations/src/clients/mooglemail/transport/outcomes/rejection.rs) 仅将闭集拒收码转为安全文案。
  14 项 runtime 模板由 application 定义、config 解析；变量兼容与同版升级见
  [邮件正文模板](ops/config/README.md#邮件正文模板)及[交付载荷契约](docs/contracts/schema.md#结算恢复与交付)。
- 附件装备准备：integrations 的 `clients/mooglemail/sender/equipment.rs` 独占 Armory 出库／解除保护编排，
  `parser/reads/inventory/` 拥有精确装备状态和 token 证明，`protocol`／`forms` 拥有固定目标与闭集动作。
  仅修改本次冻结发送集合内的装备；原 sending fence、凭据独占、限速及 Sent 证明不变。
  不增加应用端口、HTTP 字段、配置或持久化对象，内部正向库存证明的语义保持不变。
- Bot 内部履约：credentials 增加全局 mail generation，integrations 联合核验身份并读取精确库存，application 编排
  内部义务与资金用途，storage `settlement_funding` 独占来源/分配/追加账本，fulfillment 独占内部满足事实，
  delivery 经资金窄端口保护现金外发；预期名、普通余额或装备详情可读都不构成证明，论坛资料 reader 复用已启用
  论坛 gateway。锁前缀与 guard 见[内部履约不变量](docs/contracts/invariants.md#i-09h-bot-内部履约与现金保护)，
  启用须先完成[停机同版升级与资金核对](ops/README.md#bot-内部履约与资金保护升级)。

## 部署档位与能力

当前只支持 PostgreSQL：敏感 TOML 的 `[database]` 或兼容 `database_url` 由 `auction-config` 校验且不能混用，
字段与 scheme 见[配置说明](ops/config/README.md)，驱动须显式启用 `postgres`。

| 能力 | 实现边界 |
| --- | --- |
| 分体拓扑 | Public、Admin、Worker 独立进程；具备并发及多进程存储边界，具体扩展需验证 |
| 竞价与生命周期 | Serializable、显式行锁、最多三次序列化冲突尝试；持久调度、慢速代理与结算 workflow |
| Public | 目录、lot、bid-log、账户及成交价格、补偿事实的权威投影；账号级登录时长（默认永久、当前会话及后续登录生效），登录用户装备标记与账号级 MM/PM 渠道偏好，统一来源按领先周期去重 |
| Admin | 用户、改名、黑名单、审计、规则、runtime、world、录入/报表及 realtime 运维 |
| 结算与外部系统 | 结算、履约、recovery、凭据、论坛、邮件与告警的独立 owner；论坛目录每帖最多 150 件，续页身份与正文由 storage 持久化；forum owner 持有终态刷新纳入标记及全固定页确认版本，结算结果仍归 settlement |
| 运维 | 显式灾备、canonical v28 接管；可选 Linux 单主机 supervisor 与确认后的专用空库初始化；单目标未发布草稿软截止修正（默认只读） |
| 邮件身份基础 | PostgreSQL `000045` 保存可选预期完整玩家名称及既有凭据版本；预期值不是身份证明 |
| Bot 可信证据 | `000046` 全局 mail generation；数据库 active 凭据、联合远端身份、精确正向库存及持有至提交的操作 guard；配置 Cookie 不伪造持久身份 |
| Bot 现金与内部履约 | `000047` 真实资产及全部责任对账、来源唯一资金账本/预留、统一现金发送门；`000048` 独立内部证明与原义务依赖原子推进；初始自有资金为 0，未对账及未知资金不开放 |
| COD 退件去重 | `000051` 由 recovery owner 追加退件消费事实，与退回计数、回收动作同事务；每封退件派生独立且可重放的投递命令 |
| 邮件详情恢复 | `000052` 由 mail-poll owner 持久保存待读邮件摘要；应用层提供窄日志端口，集成层读取详情前入队，完成前缀与入站事实同事务移除 |
| 邮件发送节奏 | `000059` 由 application/delivery 定义七类间隔，storage/delivery_scheduler 持久化分组发送起点、fence 内串行复核；jobs 消费提交后唤醒并优先领取超价提醒，Admin runtime 编辑、Worker 重启生效 |
| 邮件维护轮转 | `000060` 由 application/mail maintenance 定义只读候选轮转，storage/mailbox_sweep 持久化 world 内 `(created_at,id)` 游标；只在逐条开始核对前推进，不把批次后缀、时间预算耗尽或观察当作业务完成；discard 另以原投递元数据的持久尝试围栏禁止未知外写重放 |
| 邮箱共享读取与发送证明 | integrations 独占 Inbox/Read Mail/Sent 的有界观察、合并读取与发送前后屏障；application/delivery 定义实际发送摘要，storage/delivery_scheduler 与 `000061` 在 Sending 同事务冻结发送尝试并唯一绑定远端 MID；普通观察不授予资金或库存写入权限 |
| 邮件运行诊断 | application 定义只读观测端口；integrations/jobs 报告真实目标，jobs 持有有界进程快照，HTTP/独立 Admin DTO 显式选择安全数字 MID；无新增迁移或业务写入，重启清空观测 |
| 装备读取诊断 | application 定义安全原因闭集，storage/mail_poll 通过 `000036` sidecar 和 `000056` 扩展 CHECK 保存首个拒收诊断；诊断不参与命令摘要、完成证明或游标身份 |
| 旧自寄履约核对 | `000053` 由 application 结算修复端口与 storage legacy repair owner 保存只追加的身份、库存和旧投递关联证明；单笔显式命令处理已核实的 Bot 自持装备，保留原发送证据与命令身份 |
| 旧实收责任接管 | `000054` 由 application funding 端口与 storage settlement funding owner 保存逐笔接管事实；同账号精确 Sent-collected 证明将已隔离责任转入对应标的用途，卖家留存不重复发行旧收款或增加自有额度 |
| Bot 仅记账策略 | `000055` 由 funding owner 持久化 UID 账户策略；显式初始化 `record_only`，按单笔冻结经济记录有限会计来源和用途，不采集或比较账号现金；默认 `strict_cash` 维持原现金门 |
| 账户读取索引 | `000057` 由 storage/account 增加用户维度的 bids 与代理授权历史索引；不新增表、业务状态、HTTP 字段或权限 |
| 投递外部引用索引 | `000062` 由 storage/delivery_scheduler 为非空 `deliveries.external_reference` 增加非唯一部分索引；加速新 MID 绑定前的历史碰撞查询，不改变 Serializable、唯一绑定或历史投递事实 |
| 卖方全额结算 | `000064` 只扩展既有拍卖/结算表约束；domain 的 `hold_curve_v2` 保证卖方等于成交价，奖池仅承担买方基础优惠，不足按全场权重同比缩减、逐件取整留池，零预算无优惠仍结算；旧快照与账本不改 |
| Take 原账号恢复证明 | `000063` 由 application/delivery 定义冻结 Take 身份，integrations 在 fence 前固定同一凭据会话，storage/delivery_scheduler 只追加原尝试；mailbox_sweep 同时核对原账号、当前 generation 与候选 fence，旧缺绑定任务保留人工核对 |

以上为源码能力，运行与独立复审状态见[状态页](docs/status.md)，输入、成功与缺失状态由
[HTTP 契约](docs/contracts/routes.md)拥有；新增持久化能力先登记范围与 owner，再更新
[对象契约](docs/contracts/schema.md)、迁移和端口。

## 核心模型与数据流

- 世界控制业务启停；用户有稳定论坛数字 `showuser` ID、可变显示名/MoogleMail 收件人、状态和版本，首次竞价可建
  `forum_only` 主体但不自动发放登录能力，会话真值保存在数据库。
- 拍卖沿 `Draft → Preview → Live → Final → Settling → Settled` 前进，只有 Draft/Preview 可取消，创建时冻结经济、
  截止、代理和结算策略，后续配置不回写快照。
- `lot_id` 是稳定身份，`lot_number` 是场内顺序，`category_key/category_number` 是公开分类编号，撤回/重挂不复用
  编号；`withdrawn` 保留事实及返还义务，论坛只渲染 `live/sold/unsold`。
- 普通出价、代理上限和慢速代理共用领域计划，新输入遵守 1000 Credits 网格和 1B 上限、历史金额不截断，规则归
  [不变量契约](docs/contracts/invariants.md)。浏览器经各自 HTTP 入口消费 contracts DTO，Worker 经 jobs 调用同一
  应用用例；缓存只加速可重建读取，不能替代事实、锁或命令账本。

## 命令、事务与事实

竞价在一个后端事务内完成 `command_id`/摘要重放检查、world/auction/lot 与资格保护、领域计划、事实、版本、追加
审计及实时事件/outbox；同 ID 不同负载冲突，已接受结果不受当前配置重解释。

| owner | 关键职责 |
| --- | --- |
| domain | `CompetitionPlan`、竞价事实/代理转换/快照一致性、失败原子性、Fast/Slow/Final 共用反超计算 |
| application | 锁内用例顺序；`legacy_ledger` 与 `slow_replay` 校验历史结果与请求的绑定 |
| storage | 后端锁模型、锁后 `occurred_at`、资格/会话一致性、generation/due fence；扫描在 LIMIT 前排除已截止 lot |

生命周期、结算和恢复共用应用入口，人工提前开放/结算默认关闭，金额守恒、连续领先时间、录入目标及追加更正见
[不变量契约](docs/contracts/invariants.md)；`000044` 由 storage/Users owner 追加时间顺序约束与有证据的审计更正，
见[数据库契约](docs/contracts/schema.md#追加迁移与历史事实)。装备标记由 application lot-watch owner 定义用例与窄
端口，domain 拥有领先周期及提醒规则，storage 拥有标记、版本、通知事实与 delivery 绑定；标记变更与竞价采用同一
lot 锁序，成功新建代理同事务默认标记。MM 经既有 delivery owner 在提交后发送，PM 由 lot-watch-pm 队列独占发送
fence 与只读对账；`000042` 的账号偏好 SQL 由 storage 独占，旧 watch.channel 恒为 mm 兼容占位，见
[超价提醒不变量](docs/contracts/invariants.md#i-09g-装备标记与超价提醒按领先周期去重)。

## Worker 与外部副作用

Worker 只负责持久调度、重试与执行时机；due、游标、attempt、lease、fencing 及命令结果由数据库持有，Final 与调度
完成 fence 同事务提交，`scheduled_rounds.schedule_snapshot` 固定自动排期的本地时区来源。关闭世界阻止新场次、不取消
已有履约/恢复，drain 后各 owner 停止领取。实际取得的邮件或论坛 credential owner 永久失效时，装配根触发有界 drain 并
非零退出，由进程管理器重建。外部副作用只在本地提交后执行：入站详情 GET 前按有效租约保存邮件摘要，
崩溃后优先按持久 MID 恢复；完整发件人及 allowlist 判定只采信详情，列表显示名仅保留快照兼容。
详情未获白名单授权时保留待读项并阻断后续处理；按连续前缀提交完成证明、cursor 并移除对应待读项，
未知写入先人工对账，不能凭本地 UUID 或成功页面重发，流拍/撤拍逐件 Send 原装备，不能 Return 已取件原邮件。

ComposeSend 先取 Sent baseline，再接受新增、唯一且精确匹配的 MID，Take/Return 各有独立动作证明；每种 Cookie 只有
一个运行 owner，明确登录失效才标 invalid。`DeliveryTransport::prepare` 在持久 sending fence 前只读检查 compose，
消耗式 prepared future 连续持有同一 credential guard，fence 成功且租约安全后才写入；脏草稿只延后对应 world 的
Send，旧 sending/unknown 不转换。截止与有界续租归 `auction-jobs::worker::runtime::delivery::lease`
（`delivery/lease.rs`），storage `delivery_scheduler/state.rs` 在行锁后复核实时租约；细节见
[不变量](docs/contracts/invariants.md)、[Worker](apps/auction-worker/README.md)、
[任务编排](crates/auction-jobs/README.md)与[外部适配](crates/auction-integrations/README.md)。

论坛目录按分类及编号稳定排序，每帖最多 150 件，从 #1 连续分页，系统日志在最后一个目录页之后，用户竞价再从下一楼
开始；历史已发布/未知主题冻结单页继续导入，全部固定帖真实 post ID 保存后才导入命令，回帖只导入普通 bid。
application 定分页端口，integrations 负责渲染/解析，jobs 负责顺序发布与对账，storage 的 `forum/catalog.rs` 持久化
续页身份与正文；`000043` 由 storage `forum` owner 登记终态刷新（历史已结算豁免）。倒计时
（`forum/render/countdown.rs`，只用既有时间且不依赖 Dioxus 资源/JavaScript/第三方服务）、拒绝提示
（`worker/forum/reply/rejection.rs`，旧诊断仅白名单映射）与 Admin 导入 gate
（`auction-web-admin::app::surface::pending`，仅 Users 启用）均不新增表或迁移，见
[图片契约](docs/contracts/routes.md#p22-自托管论坛倒计时)、
[外部副作用契约](docs/contracts/invariants.md#i-08-外部写入发生在本地提交之后)与
[Admin 设计](crates/auction-web-admin/DESIGN.md)。
主题状态由 storage 同源快照投影，integrations 生成有界标题及精确历史候选，jobs 在一次论坛目录读取中对账并同步
首帖元数据；取消仅刷新已有有效主题，不创建或继续导入竞价。完成证明复用终态版本字段，见上述外部副作用及 schema 契约。

## 配置与数据保护

三角色共享非敏感公共 TOML，分别引用敏感文件，Admin/Worker 可共享 runtime。runtime 保存由 `auction-config` 使用文件
锁、fingerprint、持久命令账本与原子替换，替换后回读或账本确认失败须向前恢复，生效须重启，Admin 恢复结果未知或需人工
核对时停止启动；数据库 `worlds.enabled` 与仅控制邮件入站的 runtime world 开关分离，字段与恢复流程见
[配置说明](ops/config/README.md)。
角色/common/secrets 的读取预算与普通文件门禁由 `auction-config::config::reader` 统一拥有；runtime 启动和管理
读取共用文件检查，管理写入身份与 root 只读部署装配分开，具体权限及大小边界见[配置手册](ops/config/README.md)。
Admin 的可选 `worker_config_file` 只由 config 读取非敏感 Worker 预算/enabled 及 runtime 同源身份，
冻结为三个标量的验证上下文；候选验证不另做 I/O。两套部署预检绑定实际 Worker 文件，并核对三角色装备密钥，
不会要求 Public 持有仅 Admin/Worker 使用的登录或外部凭据密钥。

| 能力 | owner 与保护边界 |
| --- | --- |
| 凭据与装备链接 | 敏感配置/专属加密适配；日志、事件及审计不记录 secret。P09/P11 核对归属后短暂解密装备 URL，响应 `no-store`，不缓存 |
| 邮件审查 | 只返回安全投影，原始异常证据独立加密，见[诊断说明](ops/mail/diagnostics.md) |
| A44 Cookie 校验 | application 前后复核版本，storage 返回信封，integrations 执行受限 HTTPS GET；不持久化结果、不取得 Worker 写入所有权、不改 active/invalid |
| Admin 会话 | 启动永久撤销旧会话，token 摘要绑定 actor/secret；数据库保存摘要与撤销状态，重启需重新登录，Public 不受影响 |
| Public 长连接会话 | 应用只读 inspect 端口与 storage 权威查询，HTTP 决定复核周期/预算；不续期，不把真值放 Hub 或缓存 |
| 缓存 | 默认关闭，仅加速 Public 规则；有限 TTL/超时/并发，miss、损坏、不可用及失效失败回源 |

鉴权与状态码见[HTTP 契约](docs/contracts/routes.md)，缓存预算见[适配器说明](crates/auction-cache/README.md)。
Cloudflare → Caddy HTTPS → Unix socket 的可信链与 XFF、Origin、Secure Cookie 判定归部署配置，`auction-http` 默认禁止
CDN 保存全部响应（仅精确静态 handler 可放行），部署规则不得覆盖源站禁止缓存头，见
[橙云部署](ops/README.md#cloudflare-整站代理)。

## 浏览器、实时与制品

Public/Admin 是分离的 Dioxus CSR SPA，各有路由、译文、产物和 HTTP 资源类型，共享层仅提供基础能力，设计见
[Public](crates/auction-web-public/DESIGN.md) 和 [Admin](crates/auction-web-admin/DESIGN.md)；五语言与 `hv.lang` 保持
稳定，列宽/截断/窄屏结构见[表格契约](docs/design/table-layouts.md)。Public `surface/pagination/` 拥有列表数量、译文、
URL 更新与 `preferences.rs` 本地偏好，shared `Pager`/`components/pager/size.rs` 只提供可选数字分页与页码限制。
Public `surface/catalog/columns/` 独占装备目录列身份、默认权重、显隐控件和浏览器偏好；仅改变展示，
不增加 HTTP 字段选择或服务端配置。保留命令宿主与表格结构，共享 `TableColumns` 仍只负责可见权重归一化。
shared `media/viewport.rs` 拥有 VisualViewport 监听及 CSS 变量生命周期，`style/base.css` 拥有公共基础样式，
`style/mobile.css` 单独维护触控字段、分页与底部弹窗。各 surface 的桌面／移动纯展示模块和独立 CSS 拥有导航与设备布局，
请求、表单、表格、身份和未决命令仍由稳定的共享业务组件持有，不因宽度变化重新挂载或重复读取。
CSS 以 `foundation, surface, desktop, mobile` 固定优先级，不依赖 Dioxus 的 head 顺序；无新增运行依赖或迁移阶段。
Public 手机业务表与装备事实由本端 `assets/mobile.css` 重排为卡片，仍使用同一业务 DOM 和五语字段名；
全页边距由 `assets/public.css` 统一拥有，具体展示与桌面／打印边界见 [Public 设计](crates/auction-web-public/DESIGN.md#3-外壳与布局)。
装备标记只允许登录用户修改，Public 按 user UUID 与 auction UUID 隔离本地缓存，服务器为唯一权威，不增加业务表列，
也不把装备链接、代理上限或凭据写入标记缓存。
Public `identity/account/watches/` 拥有“我的”当期标记列表，组合既有当前场次、账号标记与完整公开目录 API，
按账号、世界和场次隔离结果；分页完整读取后复核场次与标记版本，再以真实交集本地分页，不新增服务端写入或持久化 owner。
同账号/世界的后台重读保留已完成列表与分页，不订阅只改变价格的普通竞价通知；身份变化立即隔离旧结果。
Public `surface/router.rs` 与 `surface/board/realtime.rs` 拥有浏览器实时启用策略，仅登录后的 `live`/`final` 场次建立连接，
预览及其他非竞价状态停用，通过既有 60 秒权威场次读取发现开拍。显式“立即刷新”先呈现忙碌反馈再整页重载，自动同步保持原地更新。
Public `surface/catalog/command_scope.rs` 与 `auction/watches/commands.rs` 在异步展示分支外持有稳定命令，
GET 错误、阶段和分页不丢失未知意图；实际拍卖查询决定 lot/显式拍卖的实时目标，不由缺省 world 猜测。
shared `transport/browser_lock.rs` 只提供原生同源独占锁和取消机制；Public `api/identity/session/cookie.rs`
拥有 Cookie 同步策略，普通 GET 不改 Cookie，所有写 Cookie 操作串行，数据库仍是唯一会话真值。
Admin API-owned 会话写门禁与认证外壳分离：临时错误保留草稿和未决状态但禁写，明确身份变化才清理。

首次设置 UI 位于 Admin crate，由独立 `auction-setup` 服务提供，不使用正式 Admin 会话或持久化凭据，端口与周排期归
setup DTO。REST/WebSocket DTO 由 contracts 拥有；通知只提示重读，
跳号/重连走 HTTP Catch-up/权威快照，当前无 SSE 或外部广播 sink，预算与关闭原因见
[事件契约](docs/contracts/events.md)。显式 `web-embed` 选择各自资源，API-only 固定空集合，`allow_missing` 保证无产物
可编译并返回 503；归档与制品规则见[部署手册](ops/README.md)与[CI 说明](ci/README.md#下载-actions-制品)。

## 生命周期与运维

`auction-process` 提供信号和有界 drain，装配根决定停止顺序：首次 SIGINT/SIGTERM 停止接收/领取，默认最多 10 秒业务
drain，第二次信号或超时强制收口，再完成心跳和有界 tracing 刷新。

| 部署 | 进程与迁移所有权 |
| --- | --- |
| 分体 | systemd 管单角色 PID/重启；`auctionctl deployment start/restart` 严格停机后迁移，按 Worker → Admin → Public 启动，反向停止。已全部运行时 start 只复核 readiness/回执 |
| 合并 | server bootstrap 管进程树、预检及次序，config 管运行目录；首次明确确认后 storage 在迁移锁内检查专用空库和零其他客户端并初始化。普通启动只验 schema，升级显式 `migrate-managed` |

分体 HTTP 共享运行根由 tmpfiles 独立预建，Worker 的 RuntimeDirectory 只拥有控制 socket 子目录；
角色重启不得清理其他角色的 HTTP socket 或实例锁，安装和旧模板切换见[部署手册](ops/README.md#systemd-服务与顺序控制)。

分体单角色停止预算 45 秒，总控及合并 unit 均为 155 秒；两种模式不得混用同一数据库，本机文件锁不证明跨主机隔离。备份、恢复、
重建、v28 接管均为独立受控命令，不进入自动启动；一次性部署码保护独立设置页，只有合并模式的额外确认才授权专用空库
初始化与启动（保留迁移兼容视图、锁和 checksum，结果未知时不得自动初始化）。门禁见[部署](ops/README.md)、
[灾备](ops/deploy/RECOVERY.md)、[v28 接管](ops/deploy/LEGACY_V28_ADOPTION.md)及[合并服务](apps/auction-server/README.md)。
