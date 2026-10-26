# 管理端浏览器界面

[文档导航](docs/README.md) · [Admin 设计](crates/auction-web-admin/DESIGN.md) · [架构](docs/design/architecture.md) · [契约](docs/contracts/README.md)

Dioxus 纯客户端渲染的管理端 SPA，拥有管理认证、查询、命令、规则与非敏感配置编辑。页面结构与工作区细节见
[Admin 设计](crates/auction-web-admin/DESIGN.md)，本页只说明实现边界、命令与资源侧约束。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/app/surface](crates/auction-web-admin/src/app/surface) | 路由、认证外壳与会话内状态 |
| [src/app/surface/pagination](crates/auction-web-admin/src/app/surface/pagination) | 每表数量 URL、本地偏好、共享 Pager 译文与请求身份门禁 |
| [src/app/surface/pending](crates/auction-web-admin/src/app/surface/pending) | 当前路由已提交命令的稀疏宿主、累计未知结果与原样重试区 |
| [src/app/surface/auth/navigation](crates/auction-web-admin/src/app/surface/auth/navigation) | `desktop.rs` 常驻侧栏、`mobile.rs` 折叠菜单与按钮；共享唯一入口清单，不持有会话、草稿或请求 |
| [src/api/pagination](crates/auction-web-admin/src/api/pagination) | 显式分页请求的安全乘法与既有服务端边界校验 |
| [runtime_config/taxonomy.rs](crates/auction-web-admin/src/api/operations/runtime_config/taxonomy.rs) | 服务端装备词典投影的界限、唯一性及引用完整性校验 |
| [src/app/auction](crates/auction-web-admin/src/app/auction) | 拍卖、lot、录入、结算/恢复页面 |
| [src/app/identity](crates/auction-web-admin/src/app/identity) | Users、改名与凭据管理 |
| [src/app/fulfillment](crates/auction-web-admin/src/app/fulfillment) | 邮件审查与 delivery 操作 |
| [src/app/operations](crates/auction-web-admin/src/app/operations) | dashboard、runtime、实时运维及首次设置 |
| [equipment_colors/options](crates/auction-web-admin/src/app/operations/config/form/equipment_colors/options) | 按服务端装备词典投影计算标色候选的筛选和去重并集 |
| [equipment_colors/selection.rs](crates/auction-web-admin/src/app/operations/config/form/equipment_colors/selection.rs) | 原生复选框多选、旧候选保留及多类别种类/词缀并集关联 |
| [src/app/content](crates/auction-web-admin/src/app/content)、[src/api](crates/auction-web-admin/src/api) | 规则工作区、同源请求与严格 DTO 解码 |
| [assets/admin.css](crates/auction-web-admin/assets/admin.css)、[desktop.css](crates/auction-web-admin/assets/desktop.css)、[mobile.css](crates/auction-web-admin/assets/mobile.css) | 分别拥有设备无关视觉、桌面布局、手机/触控布局；三份独立静态资源按 CSS layer 排序 |
| [i18n](crates/auction-web-admin/i18n) | Admin 独立维护的五语言译文 |

## 职责与实现边界

正式 Admin 有 21 条浏览器路径，与[后端入口清单](crates/auction-http/src/assets/routes.rs)和
[HTTP 契约](docs/contracts/routes.md)同步；额外 `/setup` 只由独立首次部署服务提供，正式 Admin 返回 404。
`main.rs` 只启动，`app/` 与 `api/` 按同一业务 owner 分组，`surface/auth/` 拆分会话、return 地址、登录和认证后外壳，
客户端未知页也在认证外壳内；页面身份、查询与响应必须绑定当前拍卖/邮件等目标 UUID。
Router 挂载前保存首次 query，在认证等待前按原路径恢复一次；保留显式筛选、数量、未知 query 与 hash，
不覆盖后续导航，也不丢弃 Router 的 history state。
`operations/setup/` 独立持有首次设置草稿、校验与提交，不复用正式 Admin 会话或 runtime 草稿：先验证部署码再开放配置，
部署码和密码只留页面内存，失效、保存成功或结果未知时清空；合并模式须明确提交 `confirm_initialize`，
保存仅提示后续初始化/启动，结果未知锁定操作并先停止进程核对目录（见[初始化契约](docs/contracts/routes.md#首次部署初始化)）。
首次设置的 HTTP Origin 仅允许 `localhost`、`127.0.0.1` 与 `[::1]`；IPv6 地址保留标准方括号，
非回环地址仍必须使用 HTTPS，输入必须是无路径、查询或片段的规范 Origin。

## 配置草稿与表格展示

Runtime 非敏感草稿和稳定重试命令保留在当前认证外壳内，站内切页保留、退出清空、不写浏览器存储；刷新/关闭提示
未保存，新 fingerprint 与草稿冲突时保留输入。本地字段错误可定位，保存确认列出实际安全值差异和重启范围，
完整候选仍交服务端校验。
会话后台复核暂时失败时保留同一认证宿主的草稿、一次性 Key 和未决命令，同时通过 API-owned 门禁及禁用控件阻断新变更；
明确未认证、成功注销或账号/CSRF 身份变化才清理或隔离旧状态。监听模式切换清空不适用字段，
确认清单展示同一候选的实际清空差异，不放宽服务器的 TCP/Unix 互斥约束。
认证身份代数通过真实 keyed iterator 销毁旧 Scope，不只修改静态组件 key；新身份重新提供 actor/CSRF context。
拍卖路由、录入 world、报告、代理历史、入站详情及邮箱反馈各按实际状态 owner 隔离，页级未决宿主不随展示切换销毁。

业务表格与 Markdown 预览均为 100% 容器宽度、独立完整百分比列宽，窄屏保留表头/行，禁止横向滚动或卡片化；
长文本通过共享提示/浮层查看（比例见[列宽登记](docs/design/table-layouts.md)）。已有分页列表统一复用共享 Pager 的
数字页码、省略号、10/20/50/100 预设及自定义正整数数量：默认 10，URL 明确数量优先、非法值回退 10，缺省读取
`hv.admin.page-size:<scope>`（不读取 Public 偏好），浏览链接不写偏好，服务端分页请求显式携带数量并核对响应，
迟到页不能用于新请求的总数纠偏（键与上限见 [Admin 设计](crates/auction-web-admin/DESIGN.md#5-表格表单与确认)）。
lot、delivery、用户、凭据、入站邮件、改名和审计表直接排序组件已拥有的当前页行集合，不为排序再深克隆一份；
稳定排序、等值行顺序、服务端分页和原有空态保持不变。
改名页面借用完成快照，仅在交给表格时复制一次当前页记录，省去中间深克隆。
结算页面先借用目录、结算与 recovery 完整快照核对请求身份，再向子组件交付结果，避免每次重绘先复制一份完整报告。
请求只订阅服务器筛选与分页身份，当前页排序不重发 GET；Outbox 的事件、失败与 lag 分区分别订阅各自查询。
更改轮询间隔不立即刷新无关列表，显式刷新和命令成功刷新仍保留。

手机竖屏与 64rem 内的触控横屏使用顶部折叠导航，切换路由自动收起；导航内按 Escape 返回触发按钮。
桌面/手机导航由独立 Rust 纯展示模块输出，CSS 只显示适用导航；手机控制区只有一个 `admin-primary-navigation` ID。
两种展示不持有 `Outlet`、认证 epoch、读取资源或业务事件工厂；主区、配置草稿、页级未决命令、登录及设置表单始终只有一份，
改宽度不切换业务组件树、不引入第二次 API 读取，也不自动关闭未知命令。
`admin.css` 的 `surface` 层保留设备无关视觉；`desktop.css`/`mobile.css` 分别承接布局，全部先声明
`foundation, surface, desktop, mobile`，不依赖 Dioxus 静态 head 的资源输出顺序，也不使用运行时 CSS `@import`。
主布局断点为 48rem，64rem 内粗指针设备同样使用手机导航；Dashboard 在 64rem 内单栏、其上双栏、100rem 起三栏。
页头、工作区、登录与首次设置保留屏幕安全区；筛选、诊断、规则、配置、确认与未决动作按可用宽度重排。
首次设置在 48rem 内使用单列表单；共享层拥有移动输入的 16px 字号和 44px 高度，Admin 负责具体表单布局。
业务表始终保持完整行列与百分比列宽；凭据、Realtime 动作布局放在单元格内部，不把 `td` 改为 flex。
复选项和诊断展开入口使用整行触控区域，长文本继续使用共享键盘/触控全文查看。

拍卖、lot、用户、凭据、投递、邮件关联任务、Recovery、业务世界启停、改名回滚、规则和 replay 在稳定宿主安装未决操作区；runtime 未决命令与草稿同属认证外壳。
已提交意图冻结完整负载、原 UUID 和版本，任务不随行、弹窗或邮件详情卸载取消；不把关闭展示解释为取消服务器请求。
同实体未决期间禁止新意图，成功移除，从未未知的明确拒绝才可显式关闭；后续 401/429 不能解除累计未知。
创建用户的 generated/manual Key 请求均保留原输入工厂供重试，成功 Key 直接交稳定页面，仅驻留内存并须安全交付，
不承诺入队邮件或 dry-run 放行。Users 创建、行命令和重试共用同步串行门禁，Key 确认前仍阻止下一条。
忙态属于稀疏条目；完成或关闭时释放工厂及状态，不逐命令分配直到页面卸载才回收的 Signal。
凭据删除与启停、改名共用实体门禁。邮箱轮询和 Dashboard 控件使用独立的稳定宿主；
卡片换邮箱或快照错误卸载控件，不丢弃已提交的 UUID、版本与负载，成功只刷新该宿主负责的读取。
Recovery 冻结动作、原因和版本；世界命令冻结 enabled/revision；改名回滚按用户 UUID 隔离，
首次准备后冻结完整 fence。未知后的 stale 拒绝不换 UUID 或重读新版本，只能原样重试。
装备标色规则以借用集合展示，单项选择只复制待修改字段；Admin 不再启用无消费者的 UUID v5 feature。
未决面板借用稀疏条目，仅复制展示文字，不为重绘复制整张集合与冻结工厂；规则和 runtime 初始化也先借用完成快照，
再按原查询/指纹门禁更新草稿。首次设置的静态字段组不逐次分配 Vec。移除未接入模块树的旧 Dashboard 指标/运行详情实现、
对应 CSS 与五语废弃消息，唯一 Dashboard 快照和当前 `content.rs` 分组保持不变。

## 国际化

业务文案通过 `t!` 读取本 crate 的五种 `i18n/*.ftl`，默认 zh-CN、编译期内嵌；新消息 ID 不含点号且同步全部语言，
避免缺键 panic。共享层只提供语言集合、`hv.lang` 与 `<html lang>` 同步。

## 构建与资源边界

```bash
bash ci/release/web-bundle.sh admin
```

该任务使用 `dx bundle`，复制到 `target/dx/build/auction-web-admin/web/public/`，供 Admin 显式 `web-embed` 制品内嵌。
与 Public 的组件、路由及产物分离，不启用 SSR/hydration，正式服务不向未知路径提供 SPA 入口（资源分派见
[auction-http](crates/auction-http/README.md#静态产物内嵌)，发布步骤见[开发指南](docs/guide/development.md#浏览器与发布构建)）。

## 管理数据与命令

所有页面读取安全 DTO，动作按服务端 capability、版本/fencing、CSRF 与稳定 `command_id` 提交；相同负载失败重试复用
原命令，成功后重读权威数据，浏览器不复制状态机、不计算结算金额、不推断外部回执。状态冲突、能力缺失、未知结果与
普通依赖错误分别展示，不用假数据填充。创建/改期首次发送冻结完整负载与命令 ID，未编辑重试不再用当前时钟否决原
请求，未编辑时间保留原 RFC3339 精度；录入表单按 world 隔离、处理中禁止切换世界，复合结果只接受匹配请求 world 的
响应。
邮件轮询状态与读取时的邮箱绑定，切换邮箱立即隐藏旧快照，操作卡片也按邮箱重建；同一邮箱后台刷新仍保留完成快照。
展示卡片可重建，已发命令不归卡片所有；离开整个页面仍结束本地追踪，不承诺跨页面或浏览器刷新持久化。
暂停/恢复反馈也归属邮箱卡片，旧请求完成不能把成功或错误写到后来选择的邮箱；未决区仍保留原邮箱和命令。
轮询查询与入站详情中的邮箱字段明确标为“邮箱键”，不再与来信发件人混淆。
[mail/progress.rs](crates/auction-web-admin/src/app/fulfillment/mail/progress.rs) 在状态卡显示已完成 MID、当前阶段/目标及最近失败定位，
[API 诊断校验](crates/auction-web-admin/src/api/fulfillment/mail/diagnostics.rs) 独立验证闭集、邮箱绑定、数字 MID 与时间。
仅加载/刷新状态时更新，不自动触发轮询、跳过或重发；交互与观测边界见[Admin 设计](crates/auction-web-admin/DESIGN.md#邮件诊断与投递)。
创建/改期和审计筛选的 `datetime-local` 按浏览器本地时区输入，提交时转换为 UTC；展示文案明确这一边界，
未编辑的既有 RFC3339 时间仍保留原精度。规则候选校验接受服务端去除标题首尾空白后的规范值，并以该值继续发布。


| 页面组 | 关键边界 |
| --- | --- |
| 拍卖/lot | 生命周期、改期、撤拍与起拍价依 capability；Final 不等于完成结算，提前结算受服务端策略控制 |
| 结算/Recovery | 先选 settled 目录，再按匹配 UUID/world/期号读取报告，确认交给服务端重验事实 |
| 邮件/Delivery | 审查快照与历史决定分开；重试只表示重新排队，未知 Send 不可凭裸引用确认成功 |
| Users/凭据 | secret 只在授权录入或登录 key 一次性响应中短暂存在，不写浏览器持久化、日志或普通列表 |
| 审计/Realtime | 审计只读；最近事件仅元数据，replay 需显式确认，不伪造未冻结外部 sink 成功 |
| Dashboard | 每次读取一份安全快照，未采集与不可用显式区分，命令接受不表示任务完成 |

Dashboard 的邮件卡片区分未知、无轮询器、降级、暂停和运行；全局实时积压使用服务端 `total_lag`，
不从有界明细推断全局健康。上述投影只供观测，不改变 Worker 或投递状态。

拍卖页 `/auctions` 的论坛状态区可选择本页拍卖并手动刷新，只读取 Admin 认证 API 的本地安全投影：
[src/app/auction/auctions/forum.rs](crates/auction-web-admin/src/app/auction/auctions/forum.rs) 拥有选择与展示，
[src/api/auction/forum.rs](crates/auction-web-admin/src/api/auction/forum.rs) 拥有 GET 和 DTO 校验，切换拍卖隔离旧请求结果。
刷新不重试发布、不推进 Worker 状态，也不能解除人工写入门禁；该区承接论坛标题、正文、源/内容版本、同步/扫描及错误
时间与错误码，正文默认折叠并以转义 BBCode 纯文本显示，明确标为本地目标镜像而非远端逐字一致证明。

`mail_disposition_conflict` 隔离只允许核对后标失败，禁止成功确认和重试。凭据更改提示 Worker 重启；A44 只保留当前页
的分范围认证观测，不能证明写权限或改变凭据状态。邮件 Cookie 导入可填可选 `player_name`（精确完整游戏名称，非数字
UID），forum Cookie 不接受该字段；可信自身身份页面协议未闭合时配置预期名称仍返回 `identity_unproven`。导入一旦结果
未知，锁定来源、kind/world/label/player_name、密码或 Cookie 原值与 UUID，只能原样手动重试，后续认证、限流或业务拒绝不解除累计未知。
既有 delivery 列表兼容独立 `lot_watch` 来源并使用五语言类型标签，只为 `lot_watch_invalidated` 的明确 failed/无 sending/
无租约/无外部引用/零 COD 投影接受 preflight 时间链且不提供重试入口（见[Admin 契约](docs/contracts/routes.md#admin)）。
准备阶段的 `failed/invalid_payload` 另要求真实 claimed/completed、有序时间与正 attempt/fencing，且无 sending/租约/
远端引用/退信责任；未知发送不能套用该例外。Storage 验证原始未写入标记，浏览器只消费安全 DTO，不伪造发送时间。
对账下拉的条件选项显式绑定页面草稿选择，保证显示的 Requeue 与实际提交的 retry 一致；
不通过无状态 Dialog 外壳的 key 重建草稿或未决命令。

`/cookies` 提供粘贴 CK 与论坛账号密码两种来源；游戏导入接受完整论坛 CK，但服务端只保存两个共享认证字段。
账号密码由服务端完成获取，不返回 CK、不保存密码、不猜测玩家名，每次只导入选定类型/世界。
密码只暂留当前表单内存，成功或离开页面清空；验证码提示使用论坛浏览器登录后粘贴，不自动重试或绕过验证。
导入等待预算独立为 20 秒，其他 API 保持 10 秒；字段、状态码及敏感边界见[外部凭据契约](docs/contracts/routes.md#外部凭据)。

装备读取的解析失败在原有诊断详情中展示九种低基数原因及原始代码，覆盖名称、品质、种类、大类及其不一致、等级、
页面结构、属性和结构化数据不一致；仅接受 `stage=parse/category=protocol_drift` 的对应原因组合，历史无明细记录继续可读。

## 规则及运行配置

规则按 locale 读取与编辑，校验/发布期间禁止语言切换，迟到结果复核目标语言及草稿；只有最近校验过的草稿可携带
revision fence 发布，冲突后重新读取。Runtime 编辑发送非敏感 JSON 候选，不在浏览器直接读写 TOML：保存使用
fingerprint 与稳定命令，六组 operations 的 managed 默认关闭，未接管候选不代表当前生效配置；经济模板/排期只影响
后续拍卖，保存后重启受影响进程、不热加载。Runtime world 开关只控制 Worker 候选，数据库业务世界由独立版本化命令
管理（见[配置手册](ops/config/README.md#runtime-共享与重启)）。
规则发布及 runtime 保存一旦未决，编辑、再次校验、重载和取消均不能替换原意图；重试仅使用已冻结工厂。
“允许手动提前开拍”只控制 Preview 提前进入 Live，不表示手动进入 Final；提前结算仍是独立开关。

邮件运行参数的七类发送间隔由 `operations/config/form/operations/mail.rs` 编辑；复用数字字段、范围提示、错误焦点、
草稿/指纹和保存确认。旧响应缺省按 0 展示，全零不额外物化；字段说明区分业务最小间距、普通轮询及 HTTP 限速，五语同步。
接管和重启范围见[发送间隔](ops/config/README.md#邮件业务发送间隔)，保存不触发发送。

参数配置的“拍卖控制”“Worker 入站与发件人收录”“装备收录”默认折叠，点击分组标题展开。

论坛标色的装备类型、装备种类、品质和前后缀均使用原生复选框多选；前三项未勾选表示全部，同一项内取并集、不同项之间取交集。
旧单值规则回填为一个已选项，保存统一写 `categories/equipment_types/qualities` 数组并清空对应旧单值；旧单值恰为材质名且
未主动编辑种类时保留精确旧值，标签明确标注，避免编辑其他字段时扩大匹配。编辑、重排、导入导出
仍使用同一完整 runtime 文档与 fingerprint/稳定命令。GET、校验及保存请求均显式携带
`include_equipment_taxonomy=true`，候选只读取响应中独立的 `equipment_taxonomy`；缺少或无效词典明确报错，
不回退浏览器旧词表。词典不进入保存文档或 fingerprint，更新推荐候选不覆盖草稿，也不造成未保存提示。
品质推荐候选仅包含 `Peerless`、`Legendary`、`Magnificent`；已有规则选中的其它品质继续显示，直到用户手动取消。
装备种类候选中，武器、法杖与盾牌显示完整种类；Cloth/Light/Heavy 仅显示服务端材质族投影中的护甲种类，
如 `Phase`、`Shade`、`Power`，并标注“全部部位”。部位词只属于这三类护甲，不把 `Phase Robe` 等组合加入新候选；
已有规则选中的护甲完整名继续显示并精确匹配。前后缀按所选大类及精确种类或护甲种类求去重并集。
本次配置中的合法有限兼容完整名由服务端单独放入 `resolved_kinds`，页面据此提供该精确种类的前后缀候选，
不在浏览器复制语法或按名称首词猜测材质。GET、校验及保存均按各自文档补充，候选不只限于已保存值；
旧投影未提供该集合时按空列表读取，确证目录仍保持独立。
刷新与外部冲突时，同一词典 revision 保留草稿仍选中的既有服务端有限种类候选，新响应同名项优先；
revision 变化则使用新词典。合并只更新候选元数据，不进入保存文档、指纹或脏状态比较。
切换分类只更新候选，所有已选种类和词缀保留原文并继续可见，包括未知、历史及暂不相容的选项，直到用户手动取消，
避免精确种类被自动清空成“全部”。后缀的可选 `the ` 前导词按服务端相同的大小写规则匹配。
展示与词典更新边界见 [Admin 设计](crates/auction-web-admin/DESIGN.md#运行配置)。

## 认证与首屏边界

写入前取得的 actor/CSRF 必须匹配当前认证宿主；其他标签页重新登录，即使 actor 相同也不能借新 token 提交旧草稿。
CSRF 复核失败触发同一会话重查并暂时禁写，不因暂时网络错误销毁原未决意图。
Basic Auth 只用于登录交换，后续使用独立会话/CSRF Cookie；站内 return 地址受限，登录后 replace 返回。业务 401 触发
统一会话复核，只有会话接口确认未认证才回登录；会话 touch 按服务端时间重新安排，绝对过期仍由服务端决定，Admin 重启
后旧会话失效。弹窗焦点、背景滚动与焦点恢复由共享层处理，页面任务和监听跟随宿主卸载。Gloo 只启用 HTTP/JSON，
不继承 Public 的 WebSocket feature；可见性等待使用 `std::future::poll_fn`。首屏超过 15 秒仅提示慢加载，
真实 WASM panic 才标记运行异常；服务端数据密钥不进入浏览器。
会话刷新显式依赖检查代数及两个截止字段，定时器不复制 actor/CSRF；固定绝对截止仍须在截止后复核。
站内 return 保留合法 query/hash，但拒绝网络路径、反斜线和返回登录自身，非法输入回到首页。
Dashboard 与 Realtime 修改自动轮询间隔或关闭轮询时，先取消旧的页面任务，再按新间隔从头计时；隐藏暂停与恢复后
继续剩余分片的策略不变，取消失效循环不取消当前已发出的数据请求。隐藏等待每个 Future 只登记一个 Waker，
重新 poll 只更新它；完成或取消立即注销，快速重新隐藏可再次登记，不等待下一次可见性事件清理旧任务。
会话与用户 Key 的浏览器镜像保留 `Debug` trait，但调试格式遮蔽 CSRF token 和一次性登录 Key，
避免绕过 contracts 的脱敏边界；这不是已发生日志泄露的证明。

## 维护与验证

```bash
cargo check --locked -p auction-web-admin --target wasm32-unknown-unknown
```

仅静态编译；默认门禁与行为测试范围见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。
路由、交互或字段变化须同步所属设计与契约，编译通过不等于浏览器验收。
