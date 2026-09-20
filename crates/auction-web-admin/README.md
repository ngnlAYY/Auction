# 管理端浏览器界面

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

纯 CSR Admin SPA，拥有管理认证、查询、命令和配置编辑。
首次设置根据 verify 的 `initialize_and_start` 区分分体保存与合并初始化：合并模式必须勾选
专用空库/启动授权，发送 `confirm_initialize`，保存后仅提示后续启动并提供管理入口，
不把保存响应当作 readiness。相关确认、代理错误与提示均覆盖五种语言。

## 本页导航

- [源码入口](#源码入口)
- [职责与实现边界](#职责与实现边界)
- [页面与源码布局](#页面与源码布局)
- [配置草稿与表格展示](#配置草稿与表格展示)
- [国际化](#国际化)
- [构建与资源边界](#构建与资源边界)
- [管理数据与命令](#管理数据与命令)
- [规则及运行配置](#规则及运行配置)
- [认证与首屏边界](#认证与首屏边界)
- [维护与验证](#维护与验证)

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/app](src/app) | 管理页面与会话内草稿 |
| [src/api](src/api) | 管理请求及严格 DTO 解码 |
| [assets](assets) | 管理端视觉资源 |
| [i18n](i18n) | 五语言 Fluent 文案 |

## 职责与实现边界

Dioxus 客户端渲染的管理端 SPA。`Route` 枚举冻结 21 条路径，并与
[`docs/contracts/routes.md`](../../docs/contracts/routes.md) 及
`auction-http/src/assets/routes.rs` 的后端入口清单同步维护。
`/setup` 初始仅展示一次性部署码输入与验证按钮；独立 `POST /api/v1/setup/verify` 返回
`verified` 后才显示数据库、管理员、基础及进阶配置。部署码验证不提交配置、不连接数据库；
可重试失败保留输入，等待时禁止重复提交。部署码与已验证状态只保存在页面内存，刷新需重新
验证；后续令牌失效会清空部署码、数据库密码、管理员密码及确认值并退回第一步，保存成功也清空
这些值并隐藏表单。数据库表单分别填写主机、端口、账号、密码和数据库名称，账号密码按原文输入。
配置步骤按数据库、管理员、基础与可折叠进阶设置分组；五语言字段说明列出格式、范围和 UTF-8
字节边界。密码确认只用于浏览器校验，不进入 DTO。检查和保存复用本地校验，错误摘要可以定位字段，
进阶错误自动展开；数值保留原始草稿，不会在解析失败时沿用旧值。修改输入清除旧检查结果，服务端
仍负责权威配置与数据库校验。三个接口使用同一传输实现，每次请求各有 10 秒总预算，并非三个步骤
合计 10 秒；保存时超时、断连或响应不可识别显示结果不确定，不声称未写入。
任一初始化接口收到 `setup_outcome_unknown`（含保存传输失败的保守分类）后，页面清空部署码及
所有密码并锁定输入、验证、检查和保存；提交回调也读取该锁，键盘提交不能绕过，错误提示不会因
输入或语言切换消失。该页面内锁没有重试或解除按钮，操作者须先停止初始化进程并核对目标目录，
不能靠刷新直接重试；前端锁不替代服务端鉴权、终态门禁和禁止覆盖。
该页只由独立一次性初始化进程提供，正式 Admin HTTP 入口仍返回 404；该页不经过
普通 Admin 会话外壳，配置校验和写入仍各自携带部署码，由服务端逐请求鉴权。

## 页面与源码布局

- [src/main.rs](src/main.rs)：薄启动入口。
- [src/api/](src/api/)：按 `auction/`、`content/`、`fulfillment/`、`identity/`、`operations/` 分组的
  安全投影、会话读取和命令请求。
- [src/app/](src/app/)：按相同 owner 分组的页面组件；拍卖列表在 `auction/auctions/` 内拆分创建、表格、
  生命周期及计划动作 `actions/{row,dialog,model,schedule}.rs` 和共享映射，结算页在 `auction/settlement/` 内拆分
  settled-only 目录、URL query、世界/期号选择、结算报告、recovery 交互和辅助映射，用户页在
  `identity/users/` 内拆分创建/key、筛选表格、用户行和状态对话框，凭据页在
  `identity/credentials/` 内拆分页面编排、安全录入、凭据表格、确认对话框和安全标签辅助，Dashboard 在
  `operations/dashboard/` 内拆分单快照页面编排、指标内容和邮件控制；lot 页在 `auction/lots/` 内拆分
  页面加载、表格编排、单行更新、代理历史和展示辅助；邮件入站页在 `fulfillment/mail/` 内拆分页面筛选、
  轮询动作、入站表格、详情投影和状态标签；`shared/` 只保存本 surface
  内真正通用的基础组件。`surface/mod.rs` 只保留 surface 装配和资源初始化，`surface/router.rs` 持有
  路由与页面入口包装器，`surface/auth/` 按会话状态编排、return 路径、重试/刷新、登录表单和认证后外壳拆分，
  分别由 `auth/{mod,redirect,session,login,shell}.rs` 持有，`surface/shared.rs` 持有跨页面状态面板、
  错误重试和时间格式化；客户端未知路由也位于 `AdminShell` layout 内，不能绕过会话加载、错误或
  登录外壳直接渲染管理端 404；realtime 页面在
  `operations/realtime/` 内拆分页面状态/replay、筛选器、事件元数据表、lag 表/分页和 failure 表/分页；delivery outbox
  在 `fulfillment/deliveries/` 内拆分页面编排、状态筛选、列表/分页、动作对话框、共享状态模型和标签辅助；
  规则页面在 `content/rules/` 内拆分页面加载、编辑 workspace、Markdown 预览、发布状态模型和字段辅助。
  首次部署由 `operations/setup.rs` 编排两步入口，`operations/setup/{draft,validation,fields,form}.rs`
  分别拥有内存草稿、输入校验、字段/错误展示和配置提交，不复用正式 runtime 草稿或 Admin 会话。
  邮件详情保留请求 intake ID 并核对响应 ID；切换收件时隐藏旧详情与投递动作，同一收件刷新
  可以继续展示上次成功快照。
- [assets/admin.css](assets/admin.css)：管理端 surface 样式。设计变量来自 `auction-web-shared`。
- `i18n/*.ftl`：5 种语言的 Fluent 译文（`en`、`zh-CN`、`zh-TW`、`ja`、`ko`），默认简体中文。

## 配置草稿与表格展示

运行配置的编辑辅助由 `operations/config/` 内的 `draft.rs`、`validation.rs`、`review.rs` 分别拥有：
非敏感草稿与稳定重试命令保留在当前认证外壳内，站内切页不会丢失，退出登录后清空；不写入
localStorage/sessionStorage。整页刷新/关闭时使用浏览器原生未保存提示；显式重新加载会先确认
丢弃草稿。返回本页遇到新指纹时保留尚未保存的输入并提示冲突，服务端版本 fence 不变。
配置数值的本地错误按现有控件 min/max 和整数输入约束生成字段旁说明与顶部可定位摘要，点击会
展开所在分组并聚焦字段；完整候选仍必须通过服务端规范化校验。保存确认逐项列出修改前/后的
安全值与实际变更数量，并提示受影响进程重启及新经济模板的适用范围。

所有管理表格与规则预览内的 Markdown 表格都采用 100% 容器宽度；每张业务表按预计内容和
动作数量独立定义完整百分比列宽，Markdown 按典型文本长度分配，见
[表格列宽登记](../../docs/table-layouts.md)。不再设置固定最小宽度、横向滚动或窄屏卡片布局。表头、状态和长文本保持截断，
通过原生提示或共享的可操作浮层查看完整内容；操作按钮在列内收缩或纵向换行。
危险按钮使用独立的深红底色，白字对比度约为 5.70:1，错误文字的原有语义色不变。

## 国际化

所有用户可见文案都经 `t!` 从 `i18n/*.ftl` 取用，组件里不得硬编码字符串。译文以
`include_str!` 编译期内嵌，首屏不额外请求语言资源。

Fluent 消息 ID 不允许包含点号，使用 `nav-home` 而不是 `nav.home`。

`t!` 在键缺失时 **panic**，因此新增文案必须同步更新全部 5 个 `*.ftl`；缺一种语言就会让该
语言下的页面崩溃。语言集合、偏好持久化和 `<html lang>` 同步由 `auction-web-shared` 提供，
本 crate 只拥有自己的译文。

## 构建与资源边界

构建：

```bash
just web-build-admin   # 通过 dx bundle 复制到稳定嵌入目录
just web-serve-admin   # 本地开发监听
```

`just web-build-admin` 内部先执行 Web 构建，再将产物复制到
`target/dx/build/auction-web-admin/web/public/`，供 Admin 装配根的
`rust-embed` 使用；浏览器标题由本 crate 的
`Dioxus.toml` 固定为 `Auction 管理站点`，静态入口通过 `/favicon.ico` 引用共享品牌图标；产物由 Admin 装配根独占的
`RustEmbed` 类型内嵌，再交给 `auction-http` 提供。依赖图不引用 Public 产物；未知路径保持 404，
不获得 Admin SPA 入口。

## 管理数据与命令

管理端与公开端是不同 surface：本 crate 独立拥有自己的组件树和路由树，不复用公开端组件，
公开端也不得依赖本 crate。拍卖列表已接入 PostgreSQL 只读安全投影、world 筛选、时间线和分页，
生命周期按钮会遵守后端返回的 capability，并使用状态版本 fence、确认对话框和动作原因；Live 状态下
“进入最终时刻”与可选的“提前结算”并列，Final 状态仅保留已启用的提前结算。前者把较晚的 lot 截止
时间收紧到默认 15 分钟最终窗口并继续允许软截止竞价，后者要求理由且由默认关闭的服务端 capability
控制，浏览器不直接推断或写入结算事实；lot 页面也已接入同一安全投影和真实分页；创建拍卖命令和管理端创建表单
已接入 PostgreSQL 完整档。Draft/Preview 行还会按服务端 `modify_schedule` capability 提供计划时间
修改弹窗；Draft 可编辑预览、开拍、结束三项，Preview 的预览时间锁定，只允许编辑尚未发生的开拍与
结束时间。输入沿用浏览器本地时间编辑并发送 UTC 时间，命令 ID 在同一版本和三项输入的失败重试中
保持稳定，成功后重新读取权威列表。
生命周期返回 `invalid_auction_state` 时展示拍卖状态或排期冲突提示，不再归入通用参数错误。
拍卖筛选栏使用轻量横向布局，手工录入使用五列/窄屏单列表单，创建弹窗字段使用稳定的纵向
布局，避免通用操作栏挤压时间和 JSON 输入；手工录入成功后再次修改任一来源或命令字段会清除旧结果，
避免把上一次提交误认为当前输入的结果；
draft pending lot 还提供起拍价修改与撤回按钮；动作使用确认对话框，携带按同一输入重试时复用的 `command_id`、快照版本和 CSRF，
成功后重新读取权威 lot 页面，并展示软截止信息、设备属性悬浮详情、金额/时间格式化和代理历史关闭/重试入口；
代理历史表补齐 caption、列标题与百分比列宽与截断提示，
不在浏览器端复制 lot 状态机。
管理端外壳使用冻结的扁平导航顺序与 `/cookies` 单一凭据入口；窄屏时导航折叠为带明确展开状态的
顶部菜单，主内容不再被固定宽侧栏挤压；不在导航中显示重复别名和
`/status` 状态入口；登录页使用无卡片边框的窄表单布局，并移除会与卡片外边距叠加的根节点
`100vh`，无错误文案时桌面和常见移动 viewport 不产生无意义页面滚动；旧别名仍可直接访问并无历史记录跳转到 canonical 地址。
拍卖与 lot 表保留状态徽章、动作组和百分比自适应列宽，避免宽表在移动端丢失字段语义。
delivery、凭据、Users、改名、realtime、邮件入站、审计和结算/Recovery 表也使用状态徽章、动作组和完整文本提示；
已完成旧投影收口的列表在成功空页仍保留表头，空态提示作为独立状态显示，页面不依赖隐藏表头
表达字段含义。
改名回滚确认框、审计筛选、拍卖开放就绪提示和金额字段使用独立的对齐样式；改名/lot 确认框插入
表格时不会再撑开空白行，窄屏审计筛选会自动收缩为两列或单列。列表中的长标识、名称和详情统一
保持单行省略，鼠标悬浮或键盘聚焦可查看完整值，不以自然换行撑高记录。
delivery 页面已接入 PostgreSQL 安全投影、状态筛选、数字分页，以及 release/retry/人工对账动作；
release/retry 使用确认对话框，manual_reconcile 隐藏主动 send 的 succeeded 选项，仅保留显式 retry/failed
管理员命令。后端拒绝 `payload.operation=send` 加裸 external_reference 的成功写回；Return/Take 仍保留独立
人工核对路径。操作员取消人工对账即显式放弃本次草稿和命令状态，普通提交失败仍保留输入与稳定
命令 ID 供重试。命令携带 `command_id`、CSRF 和 fencing version，服务端仍负责最终校验，浏览器不推断外部回执。
收件详情新增“关联邮件任务”表，组件位于 `fulfillment/mail/deliveries.rs`，使用 `admin-mail-deliveries` 列宽登记。
表格按 operation、recipient、id、status、attempt、failure、actions 展示；`Send`/`Return`/`Take` 的按钮文案分别为
“重新发送”/“重新退回”/“重新领取”。仅 `failed`/`manual_reconcile` 可操作，确认后复用 delivery retry/reconcile
的 `action=retry`，携带 `command_id`、CSRF 和 `expected_fencing_version`；成功只提示重新排队，结果未知提示先核对 MM。
数据仅在收件详情中从有界 `metadata.delivery_ids` 批量读取，不把 payload、装备 key 或正文送入页面；历史收件决定不重放。
投递状态筛选使用轻量横向控件，窄屏时切换为全宽纵向控件；列表和外部回执状态展示保持不变。
Realtime 运维页面已接入最近事件元数据、lag/failure 安全投影、stream/kind/consumer/code/resolved
筛选、分页和明确空结果状态；概览默认显示事件、消费流与未解决异常总数，consumer lag 不再隐藏在
右上角弹层；事件概览总数不受列表筛选影响，显式刷新会让三类列表回到第一页。已解决 failure 不允许再次发起 replay。replay 使用
带遮罩的确认对话框，提交期间锁定输入并保留失败反馈；公开 payload 由操作员输入，服务端负责
schema、幂等、审计和事件关系校验。
`/status` 持有静态产品说明，不创建 live/ready 请求；登录外壳仍单独读取 `/health/ready`，
其 503 按未就绪显示，不与 Dashboard 运维投影混用。
Realtime 页面使用与其他运维列表一致的概览卡、分区标题和静态筛选布局；最近事件只展示安全元数据，
事件 payload、内部来源、lag/failure 投影、分页、replay 确认和外部广播 sink 边界保持不变。
审计页面已接入追加式安全记录查询、操作者/实体/来源/结果筛选、UTC 时间范围、有界数字分页和
加载失败重试；筛选草稿只在提交表单时触发查询，不会在每次按键时制造请求风暴；metadata 只作为
JSON 文本展示，不提供历史编辑。
审计筛选把操作者 ID、实体 `类型:ID`、动作和请求 ID 作为常用精确条件，把操作者类型、来源、结果和
UTC 时间收进可展开的高级条件，并提供一键清除；宽屏四列、窄屏两列或单列。审计记录仍为只读安全
投影；操作者和实体分别合并为 `ID(类型)` 的单行值，长 ID/metadata 可截断查看且不再把所有 metadata
误标为错误色。记录表使用百分比列宽和语义状态标签，窄屏继续保留表头和行结构。
结算页面先通过独立 A42 目录读取已结算拍卖：上方可按 world 与期号切换并分页，默认选择服务端稳定
排序的第一页第一项；下方只按选中 UUID 读取结算/recovery 报告。world、sequence、auction 与目录页
写入 URL，其中 auction 只与 sequence 成对恢复；用户切换建立可返回的 history 项，内部规范化只替换当前项；报告必须与目录中的
UUID/world/sequence 精确匹配后才展示。所有屏幕均按“报表选择在上、报表数据在下”纵向排列，
内容随页面自然滚动；报表选择内的世界与结算期号始终同排，扣除间距后分别占可用宽度的 30% 与 70%，
下拉框随列宽收缩，不使用按语言固定的宽度。lot 表直接展示服务端投影的持有比例、调整比例和挂牌价，并以明确文字及正负号区分折扣/惩罚，不靠颜色传达含义；
recovery 确认携带报告中的 state version、CSRF 和按同一输入重试复用的 command ID，服务端仍负责证据重校验、幂等和
事务结果，页面不在浏览器端计算结算金额或改变 recovery 状态。
`/outbox` 页面已接入 realtime 最近事件元数据、failure、consumer lag 和安全 replay 投影；未冻结的
外部广播 sink 不会被页面伪造成已发送或已成功。
Dashboard 和 Outbox 的自动轮询可见性监听由各页面持有，页面卸载时移除监听并释放闭包，
反复切换路由不会累积 `visibilitychange` 回调。
规则加载只展示与当前 locale 对齐的已完成响应；校验或发布期间禁用全部语言切换，
异步校验回写前再次核对目标语言与草稿，防止迟到响应覆盖其他语言内容。

## 规则及运行配置

规则页面已接入按 locale 读取、语言标签、动态新增 locale、版本事实、候选规范化校验、
轻量 Markdown 预览和带 revision fence 的发布表单；只有最近一次已校验的草稿才允许发布，
发布会复用稳定的 command ID，并对版本冲突提供重新加载反馈。浏览器只发送版本化 DTO，不持有
数据库或凭据状态。
规则编辑器使用事实栏间距、36rem Markdown 编辑区、48rem 预览上限和窄屏单列切换；
校验、发布门槛与 revision fence 不变。
运行配置页面已接入非敏感完整文档读取、类型化分组表单、服务端候选规范化校验、fingerprint
版本 fence、稳定 command ID、可见的加载失败重试、冲突反馈和重启提示；表单覆盖 main/isekai、
发件人 allowlist、装备过滤、装备来源、监听/安全/会话、可观测性和 14 个通知模板，并用启用 world、
装备过滤与配置指纹事实卡提供摘要。runtime 中的 world 控件明确标为 Worker/MoogleMail 入站配置；
同页另由 A40/A41 展示并修改数据库权威业务 world 状态。业务关闭要求 revision fence 和确认，遇到
未终态拍卖或活动调度轮次会显示稳定冲突，不会把 runtime 文件开关误报成业务已关闭。浏览器不直接编辑或读取 TOML；服务端校验后以锁、临时文件、
`fsync` 和 `rename` 原子替换 `runtime_config_file` 指向的配置文件。更新不会被页面误报为热加载，
保存后提示重启受影响的 Admin 与 Worker。
新增运行参数由 `operations/config/form/operations/` 按邮件、论坛、经济模板、周排期、告警和 Worker
循环分组展示；`mail.rs` 提供邮件调度、读取预算与处理/重试三组字段，`operational_fields.rs` 统一持有新增标量路径；HTTP 客户端的
`runtime_config/operations.rs` 校验对应 JSON 闭集。每组“使用本页配置”默认关闭，显示沿用部署
配置；开启后才编辑候选。未接管组的未完成输入不会阻止保存其它组，也不会覆盖原候选。
新底价按 1K 网格且最高 1B，周排期提供本地时间与五语言星期选择，Worker 子组区分生命周期、
投递、履约与恢复的独立参数。保存仍使用既有服务端校验、确认弹窗、稳定命令 ID 和版本冲突处理；
页面明确区分候选与当前生效值，并说明新经济模板只用于之后创建的拍卖。
邮件批次总预算 `batch_timeout_seconds` 同步进入表单、标量路径和五语言文案；后端省略默认值时
按 600 秒显示和校验，保存非默认值后在重启时生效，不改变旧文档的指纹计算。
TCP listener 不使用的 runtime 目录字段按服务端契约接受空值或普通文本，避免合法配置响应被误判
为加载失败；Unix listener 仍要求规范绝对目录。
邮件页面已接入 A16-A19 的轮询状态、入站安全摘要和暂停/恢复动作；动作携带 CSRF、当前
状态版本与命令 ID。页面首次打开会聚合全部已配置邮箱的入站记录，输入具体 mailbox 后再
读取该邮箱的轮询状态；入站列表和轮询状态读取失败都保留当前查询并提供原位重试；入站列表按旧页面支持 world、kind、status 筛选和 10 条分页
和安全元数据详情；列表显示 sender/subject scalar 与是否存在原始信息投影的 `has_message_display`，
详情显示 sender/subject、脱敏正文、附件外部 ID、Credits/COD；装备 key、认证邮件正文、Cookie、轮询游标
和外部凭据不进入页面；被拒绝装备邮件详情还显示 equipment ID、1-based 附件位置、stage/category、可选
detail_code、真实 HTTP status、耗时、evidence UUID 与 retention status/error。A20 触发按钮已由
HTTP/Worker 链路接入，使用单个 command ID 唤醒全部已装配 poller，返回的是接受状态，不代表业务邮件处理完成。管理总览会展示全部 mailbox
的轮询状态，并提供逐邮箱暂停/恢复与全局立即轮询操作。管理命令统一显示忙碌、禁用重复提交并以
成功/失败语义反馈；全部 Admin API 会先解析稳定错误 code，再通过五语言 Fluent 映射 stale version、
command conflict、权限、限流和依赖错误，未知 code 才使用兜底，不展示服务端原始错误。左侧导航使用冻结的扁平顺序，`/status` 等运维入口仍可通过直接 URL 访问，历史别名
会无历史记录跳转到 canonical 地址。
用户页面已接入 A28/A31 的安全用户列表、创建、改名、筛选分页、黑名单历史/次数/创建时间展示、论坛账号 ID 修改、key 撤销/重签和黑名单
操作；创建用户使用对话框，自动生成和重签的登录 key 使用独立的一次性展示框；论坛账号 ID、key 和黑名单动作使用确认对话框并要求黑名单原因；A30 回滚命令已接入 HTTP/API owner，
改名历史页面提供 A29 的名称筛选表格和 A30 回滚按钮；名称变化后立即刷新第一页，
不增加 user ID 查询控件。
页面只持有
安全用户投影，生成 key 仅在创建/重签成功响应中短暂展示，不保存数据库状态或外部凭据。
Users 页使用标题与创建入口顶栏，创建按钮不再单独占用一行；窄屏时顶栏改为纵向排列，
创建对话框、一次性 key 展示和命令幂等边界保持不变。
用户行操作由页面级 busy 信号的宿主持有任务；分页或浏览器历史切换移除行时，结果反馈、刷新和
生成 key 仍交给原页面，旧行的对话框与命令状态只在信号存活时清理。离开页面会取消任务，
共享动作 helper 在正常结束及取消时均释放 busy，不创建跨页面或身份的全局任务。
凭据页面已接入 A32 的安全列表、固定分页、导入、对话框改名、启停和带确认的删除操作；列表按冻结契约不提供额外筛选栏；
secret 只通过当前表单请求提交，不会进入响应、持久化状态或页面回显；请求失败时保留当前表单以便用同一 command ID 重试。该页面仅在 PostgreSQL 完整档并配置独立
`credential_data_key` 时可用，修改成功后提示 Worker 重启并重新加载唯一 active Cookie。
`mail_cookie` 与 `forum_cookie` 各自全局最多一条 active 记录；导入或重新启用第二条时返回冲突，
需要先停用旧记录并重启 Worker 完成所有权交接。
凭据表还恢复旧页面实际合成的 last checked、last used、imported by 三列：分别使用现有
`rotated_at`、明确空值和当前 Admin session actor；它没有虚构后端使用记录，也没有扩展 secret DTO，
state version 只参与写命令 fence，不扩张已发布的八列表格。
凭据行按稳定 credential ID 绑定组件身份，当前页排序不会转移对话框、草稿、忙碌状态或反馈；
启停和改名重试的命令签名同时绑定 credential ID、版本与动作负载。
Delivery 解码同时接受告警在激活前确定终止的窄失败形状：特定 origin/reason/error code、零
attempt/fencing、空执行时间和有效完成时间；其余失败投递仍要求完整发送轨迹。
管理总览每次只请求一份完整安全 dashboard snapshot，并提供显式刷新动作；页面按冻结顺序展示 health/process、Worker、
tracing、实时 lag/failure、MoogleMail 轮询、凭据 availability、delivery、fulfillment、recovery
和 alerting 水位；拍卖、投递列表和邮件入站不再创建额外 Dashboard 请求。页面按完整业务分组紧凑分栏，
宽屏两栏、大屏三栏、窄屏单栏；常用入口、Worker 状态和邮件动作集中在顶部。拍卖、邮件、投递、
realtime outbox 与参数配置均为仅导航入口。快照没有采集的 outbox retry/claimed/attempts 移到可展开
说明中并明确标为“未采集”，不占据主要指标卡；审计说明和告警历史时间也可展开查看。
仪表盘的自动刷新选择器与刷新快照按钮位于同一行，窄屏可整组移到标题下方；保留 URL 轮询状态和
页面隐藏时暂停的语义。分组内指标以 7.5rem 为自动排列基准，摘要卡片为 12rem；各项均可缩至容器
宽度。状态标签、错误反馈和可访问名称仍保留，邮件命令的 ID、冲突刷新和持久化语义不变。
Dashboard section 与嵌套 data 使用 v1 严格字段闭集；扩展凭据 scope 和内部诊断不会原地进入 v1
响应。MoogleMail `main`/`isekai` availability 按冻结语义分别呈现，避免其他凭据掩盖邮件 world
缺少 active 凭据；配置 `dashboard_mailboxes` 时服务端会核对所有已配置 mailbox 是否都有轮询器。
邮件页的筛选栏、邮箱加载控件和轮询状态卡保留轻量横向间距；窄屏时自动切换为纵向表单，
不会让邮箱输入或闭集筛选压缩表格字段。
凭据页的导入区使用 32rem 纵向表单和页面标题顺序；列表不增加契约未登记的筛选栏，
secret、状态动作和 PostgreSQL 完整档边界不变。
页面提供逐 mailbox 的运维动作、显式的不可用状态、手动刷新和常用运维页面
快捷入口；总览不会用前端假数据填充尚未接入的能力。

## 认证与首屏边界

Admin 登录页面已接入已有的 Basic Auth 登录交换、双 Cookie 会话读取、CSRF 注销和会话失效后的
安全回退；未认证访问深链接时会保存受限的站内 `return` 地址，登录成功后无历史记录返回原页面；
浏览器还会按服务端 idle expiry 触发会话 touch，失败时按间隔重试；绝对过期仍由服务端决定。
浏览器只保存 actor、CSRF token 和过期时间，不保存数据库或外部凭据。
登录页与已认证外壳还会读取 `/health/ready` 并提供屏幕阅读器可见的 checking/ready/degraded
状态提示；该提示保持视觉隐藏，也不改变 `/status` 的静态说明职责。
所有 `aria-modal` 管理弹窗还由壳层提供初始 `autofocus`/首控件聚焦、Tab/Shift+Tab 焦点循环、
背景滚动锁定和关闭后触发控件焦点恢复，避免键盘焦点离开当前弹窗或在关闭后丢失。用户行与凭据行
的局部动作反馈按成功/失败分别使用 `status`/`alert`，并以行名称组成可访问标签和描述关系；认证后
真正换页会聚焦主内容，首屏和同路由刷新不会抢占当前控件。
管理命令必须走版本化 REST 契约，不在浏览器端重建状态机；凭据与密钥材料不进入浏览器。
[src/api/](src/api/) 的 HTTP/stream wire DTO 已直接复用 `auction-contracts`；本 crate 只保留分页展平、
Dashboard section 解码、表单与交互等浏览器展示状态。

首屏等待超过 15 秒仅提示加载较慢，真实 WASM panic 才显示运行异常；失败标记不会被异步就绪回调
覆盖。该区分由共享 bootstrap 与两端静态入口共同维护，不把慢请求直接描述为网络故障。

## 维护与验证

修改本模块时同步对应[契约](../../docs/contracts/README.md)与源码入口；跨模块理由按[决策记录规则](../../docs/documentation.md)维护。

```bash
cargo check --locked -p auction-web-admin --target wasm32-unknown-unknown
```

该命令只做静态编译；完整门禁见[开发指南](../../docs/development.md)，运行证据边界见[验证状态](../../docs/status.md)。
