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
| [src/api/pagination](crates/auction-web-admin/src/api/pagination) | 显式分页请求的安全乘法与既有服务端边界校验 |
| [runtime_config/taxonomy.rs](crates/auction-web-admin/src/api/operations/runtime_config/taxonomy.rs) | 服务端装备词典投影的界限、唯一性及引用完整性校验 |
| [src/app/auction](crates/auction-web-admin/src/app/auction) | 拍卖、lot、录入、结算/恢复页面 |
| [src/app/identity](crates/auction-web-admin/src/app/identity) | Users、改名与凭据管理 |
| [src/app/fulfillment](crates/auction-web-admin/src/app/fulfillment) | 邮件审查与 delivery 操作 |
| [src/app/operations](crates/auction-web-admin/src/app/operations) | dashboard、runtime、实时运维及首次设置 |
| [equipment_colors/options](crates/auction-web-admin/src/app/operations/config/form/equipment_colors/options) | 按服务端装备词典投影计算标色候选的筛选和去重并集 |
| [equipment_colors/selection.rs](crates/auction-web-admin/src/app/operations/config/form/equipment_colors/selection.rs) | 原生复选框多选、旧候选保留及多类别种类/词缀并集关联 |
| [src/app/content](crates/auction-web-admin/src/app/content)、[src/api](crates/auction-web-admin/src/api) | 规则工作区、同源请求与严格 DTO 解码 |
| [assets](crates/auction-web-admin/assets)、[i18n](crates/auction-web-admin/i18n) | Admin 样式与五语言译文 |

## 职责与实现边界

正式 Admin 有 21 条浏览器路径，与[后端入口清单](crates/auction-http/src/assets/routes.rs)和
[HTTP 契约](docs/contracts/routes.md)同步；额外 `/setup` 只由独立首次部署服务提供，正式 Admin 返回 404。
`main.rs` 只启动，`app/` 与 `api/` 按同一业务 owner 分组，`surface/auth/` 拆分会话、return 地址、登录和认证后外壳，
客户端未知页也在认证外壳内；页面身份、查询与响应必须绑定当前拍卖/邮件等目标 UUID。
`operations/setup/` 独立持有首次设置草稿、校验与提交，不复用正式 Admin 会话或 runtime 草稿：先验证部署码再开放配置，
部署码和密码只留页面内存，失效、保存成功或结果未知时清空；合并模式须明确提交 `confirm_initialize`，
保存仅提示后续初始化/启动，结果未知锁定操作并先停止进程核对目录（见[初始化契约](docs/contracts/routes.md#首次部署初始化)）。
首次设置的 HTTP Origin 仅允许 `localhost`、`127.0.0.1` 与 `[::1]`；IPv6 地址保留标准方括号，
非回环地址仍必须使用 HTTPS，输入必须是无路径、查询或片段的规范 Origin。

## 配置草稿与表格展示

Runtime 非敏感草稿和稳定重试命令保留在当前认证外壳内，站内切页保留、退出清空、不写浏览器存储；刷新/关闭提示
未保存，新 fingerprint 与草稿冲突时保留输入。本地字段错误可定位，保存确认列出实际安全值差异和重启范围，
完整候选仍交服务端校验。

业务表格与 Markdown 预览均为 100% 容器宽度、独立完整百分比列宽，窄屏保留表头/行，禁止横向滚动或卡片化；
长文本通过共享提示/浮层查看（比例见[列宽登记](docs/design/table-layouts.md)）。已有分页列表统一复用共享 Pager 的
数字页码、省略号、10/20/50/100 预设及自定义正整数数量：默认 10，URL 明确数量优先、非法值回退 10，缺省读取
`hv.admin.page-size:<scope>`（不读取 Public 偏好），浏览链接不写偏好，服务端分页请求显式携带数量并核对响应，
迟到页不能用于新请求的总数纠偏（键与上限见 [Admin 设计](crates/auction-web-admin/DESIGN.md#5-表格表单与确认)）。
lot、delivery、用户、凭据、入站邮件、改名和审计表直接排序组件已拥有的当前页行集合，不为排序再深克隆一份；
稳定排序、等值行顺序、服务端分页和原有空态保持不变。

手机竖屏与 64rem 内的触控横屏使用顶部折叠导航，切换路由自动收起；导航内按 Escape 返回触发按钮。
页头、工作区、登录与首次设置保留屏幕安全区，表外筛选、诊断、配置和确认动作按可用宽度重排。
复选项及细小详情按钮使用整行触控区域；用户黑名单历史支持点击或键盘激活后打开可滚动详情。
移动布局继续使用同一套管理命令与草稿，完整行列和列宽比例保持不变。

拍卖、lot、用户和凭据页面在稳定宿主安装未决操作区：行卸载后已提交命令仍保留原 UUID、冻结参数与页级任务，仅存真实
已提交意图，同实体有未决命令时禁止新建；成功移除，首次明确拒绝可显式关闭，曾有未知结果的不因后续 401/429 释放。
录入只冻结选定来源的已提交装备参数，用户一次性 Key 直接交付页面。Users 页的创建、行命令首次提交和未决重试共享
同步串行门禁：spawn 前占用，有请求在途或 Key 尚未关闭时禁止下一条。
忙态直接属于未决条目，不为每次提交创建页级 Signal；完成或关闭条目时同时释放其状态，
不会把已完成命令的信号分配保留到离开页面。

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

拍卖页 `/auctions` 的论坛状态区可选择本页拍卖并手动刷新，只读取 Admin 认证 API 的本地安全投影：
[src/app/auction/auctions/forum.rs](crates/auction-web-admin/src/app/auction/auctions/forum.rs) 拥有选择与展示，
[src/api/auction/forum.rs](crates/auction-web-admin/src/api/auction/forum.rs) 拥有 GET 和 DTO 校验，切换拍卖隔离旧请求结果。
刷新不重试发布、不推进 Worker 状态，也不能解除人工写入门禁；该区承接论坛标题、正文、源/内容版本、同步/扫描及错误
时间与错误码，正文默认折叠并以转义 BBCode 纯文本显示，明确标为本地目标镜像而非远端逐字一致证明。

`mail_disposition_conflict` 隔离只允许核对后标失败，禁止成功确认和重试。凭据更改提示 Worker 重启；A44 只保留当前页
的分范围认证观测，不能证明写权限或改变凭据状态。邮件 Cookie 导入可填可选 `player_name`（精确完整游戏名称，非数字
UID），forum Cookie 不接受该字段；可信自身身份页面协议未闭合时配置预期名称仍返回 `identity_unproven`。导入一旦结果
未知，锁定 kind/world/label/player_name/secret 原值与 UUID，只能原样重试，后续认证、限流或业务拒绝不解除累计未知。
既有 delivery 列表兼容独立 `lot_watch` 来源并使用五语言类型标签，只为 `lot_watch_invalidated` 的明确 failed/无 sending/
无租约/无外部引用/零 COD 投影接受 preflight 时间链且不提供重试入口（见[Admin 契约](docs/contracts/routes.md#admin)）。

装备读取的解析失败在原有诊断详情中展示九种低基数原因及原始代码，覆盖名称、品质、种类、大类及其不一致、等级、
页面结构、属性和结构化数据不一致；仅接受 `stage=parse/category=protocol_drift` 的对应原因组合，历史无明细记录继续可读。

## 规则及运行配置

规则按 locale 读取与编辑，校验/发布期间禁止语言切换，迟到结果复核目标语言及草稿；只有最近校验过的草稿可携带
revision fence 发布，冲突后重新读取。Runtime 编辑发送非敏感 JSON 候选，不在浏览器直接读写 TOML：保存使用
fingerprint 与稳定命令，六组 operations 的 managed 默认关闭，未接管候选不代表当前生效配置；经济模板/排期只影响
后续拍卖，保存后重启受影响进程、不热加载。Runtime world 开关只控制 Worker 候选，数据库业务世界由独立版本化命令
管理（见[配置手册](ops/config/README.md#runtime-共享与重启)）。

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

Basic Auth 只用于登录交换，后续使用独立会话/CSRF Cookie；站内 return 地址受限，登录后 replace 返回。业务 401 触发
统一会话复核，只有会话接口确认未认证才回登录；会话 touch 按服务端时间重新安排，绝对过期仍由服务端决定，Admin 重启
后旧会话失效。弹窗焦点、背景滚动与焦点恢复由共享层处理，页面任务和监听跟随宿主卸载。Gloo 只启用 HTTP/JSON，
不继承 Public 的 WebSocket feature；可见性等待使用 `std::future::poll_fn`。首屏超过 15 秒仅提示慢加载，
真实 WASM panic 才标记运行异常；服务端数据密钥不进入浏览器。
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
