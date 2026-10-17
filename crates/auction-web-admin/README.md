# 管理端浏览器界面

[文档导航](../../docs/README.md) · [Admin 设计](./DESIGN.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

Dioxus 纯客户端渲染的管理端 SPA，拥有管理认证、查询、命令、规则与非敏感配置编辑。

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
| [src/app/surface](src/app/surface) | 路由、认证外壳与会话内状态 |
| [src/app/surface/pagination](src/app/surface/pagination) | 每表数量 URL、本地偏好、共享 Pager 译文与请求身份门禁 |
| [src/app/surface/pending](src/app/surface/pending) | 当前路由已提交命令的稀疏宿主、累计未知结果与原样重试区 |
| [src/api/pagination](src/api/pagination) | 显式分页请求的安全乘法与既有服务端边界校验 |
| [src/app/auction](src/app/auction) | 拍卖、lot、录入、结算/恢复页面 |
| [src/app/identity](src/app/identity) | Users、改名与凭据管理 |
| [src/app/fulfillment](src/app/fulfillment) | 邮件审查与 delivery 操作 |
| [src/app/operations](src/app/operations) | dashboard、runtime、实时运维及首次设置 |
| [src/app/content](src/app/content)、[src/api](src/api) | 规则工作区、同源请求与严格 DTO 解码 |
| [assets](assets)、[i18n](i18n) | Admin 样式与五语言译文 |

## 职责与实现边界

正式 Admin 有 21 条浏览器路径，与[后端入口清单](../auction-http/src/assets/routes.rs)和[HTTP 契约](../../docs/contracts/routes.md)同步。
额外 `/setup` 只由独立首次部署服务提供，正式 Admin 返回 404；catch-all 不计入路径数。

首次设置先验证部署码再开放配置；部署码和密码只留在页面内存，失效、保存成功或结果未知时清空。
各接口逐请求鉴权，页面已验证状态不能代替服务端门禁。合并模式须明确提交 `confirm_initialize`，
保存仅提示后续初始化/启动，不宣称 readiness；结果未知锁定操作，先停止进程并核对目录。
详细字段和流程见[初始化契约](../../docs/contracts/routes.md#首次部署初始化)。

## 页面与源码布局

`main.rs` 只启动，`app/` 与 `api/` 按同一业务 owner 分组；`surface/auth/` 拆分会话、return 地址、登录和认证后外壳。
客户端未知页也在认证外壳内。页面身份、查询与响应必须绑定当前拍卖/邮件等目标 UUID，切换目标不能保留旧操作入口。

`operations/setup/` 独立持有首次设置草稿、校验、字段和提交，不复用正式 Admin 会话或 runtime 草稿。
页面结构与业务工作区详见 [Admin 设计](./DESIGN.md)，不在 README 重复视觉规格。

## 配置草稿与表格展示

Runtime 非敏感草稿和稳定重试命令保留在当前认证外壳内，站内切页保留、退出清空，不写浏览器存储。
刷新/关闭提示未保存，显式重载须确认丢弃；新 fingerprint 与草稿冲突时保留输入，不覆盖服务端门禁。
本地字段错误可定位，保存确认列出实际安全值差异和重启范围，完整候选仍交服务端校验。

业务表格与 Markdown 预览均为 100% 容器宽度、独立完整百分比列宽；窄屏保留表头/行，禁止横向滚动或卡片化。
长文本通过共享提示/浮层查看，动作在列内换行；比例见[列宽登记](../../docs/table-layouts.md)。

已有分页列表统一复用共享 Pager 的数字页码、省略号、10/20/50/100 预设及自定义正整数数量。
默认 10；URL 明确数量优先，非法值回退 10，缺省读取 `hv.admin.page-size:<scope>`，不读取 Public 偏好。
浏览链接不写偏好，只有用户有效选择才保存；URL 固定有效数量，保存新偏好不能污染返回历史。
筛选、当前页排序和未知参数保留，同页列表使用独立数量键。具体键与上限见 [Admin 设计](DESIGN.md#5-表格表单与确认)。
服务端分页请求显式携带数量并核对响应；迟到页不能用于新请求的总数纠偏或显示旧页动作。
完整结算/恢复报告及收件关联投递仅本地切片，总数为真实行数，空表仍提供数量选择。

拍卖、lot、用户和凭据页面在稳定宿主安装未决操作区：行卸载后，已提交命令仍保留原 UUID、
冻结参数与页级任务。仅存真实已提交意图，同实体有未决命令时禁止新建；重试由操作员在常驻区发起，
不重读当前行版本。成功移除；首次明确拒绝可显式关闭，曾有未知结果的命令不因后续 401/429 而释放。
录入只冻结选定来源的已提交装备参数；用户一次性 Key 直接交付页面，不进入集合。Users 页的创建、
行命令首次提交和未决重试共享同步串行门禁：spawn 前占用，有请求在途或 Key 尚未关闭时禁止下一条，
结果未知保留原命令供手动重试，不自动排队。其他页面维持原并发策略。离开路由或登出取消
页宿主并释放参数，不自动重放、不跨路由缓存、不写本地存储；这不是服务端取消证明。

## 国际化

业务文案通过 `t!` 读取本 crate 的五种 `i18n/*.ftl`，默认 zh-CN，编译期内嵌。
新消息 ID 不含点号且同步全部语言，避免缺键 panic；共享层只提供语言集合、`hv.lang` 与 `<html lang>` 同步。

## 构建与资源边界

```bash
just web-build-admin
```

该任务使用 `dx bundle`，复制到 `target/dx/build/auction-web-admin/web/public/`，供 Admin 显式 `web-embed` 制品内嵌。
与 Public 的组件、路由及产物分离，不启用 SSR/hydration。正式服务不向未知路径提供 SPA 入口；
资源分派见 [auction-http](../auction-http/README.md#静态产物内嵌)，发布步骤见[开发指南](../../docs/development.md#浏览器与发布构建)。

## 管理数据与命令

拍卖页 `/auctions` 的论坛状态区可选择本页拍卖并手动刷新，只读取 Admin 认证 API 的本地安全投影。
[src/app/auction/auctions/forum.rs](src/app/auction/auctions/forum.rs) 拥有选择与展示，
[src/api/auction/forum.rs](src/api/auction/forum.rs) 拥有 GET 和 DTO 校验；切换拍卖隔离旧请求结果。
刷新不重试发布、不推进 Worker 状态，也不能解除人工写入门禁；公开页不再提供此按钮。
该区承接论坛标题、正文、源/内容版本、同步/扫描及错误时间和错误码。正文默认折叠，以转义 BBCode 纯文本显示，
明确标为本地目标镜像而非远端逐字一致证明；沿用既有 DTO，不改变 Public API 字段。

所有页面读取安全 DTO，动作按服务端 capability、版本/fencing、CSRF 与稳定 `command_id` 提交。
相同负载失败重试复用原命令，成功后重读权威数据；浏览器不复制状态机、计算结算金额或推断外部回执。
状态冲突、能力缺失、未知结果与普通依赖错误分别展示，不用假数据填充。
创建/改期首次发送冻结完整负载与命令 ID；未编辑重试不再用当前时钟否决原请求，最终仍由服务端查账本和校验。
未编辑时间保留原 RFC3339 精度。录入表单按 world 隔离、处理中禁止切换世界，复合结果只接受匹配请求 world 的响应。

| 页面组 | 关键边界 |
| --- | --- |
| 拍卖/lot | 生命周期、改期、撤拍与起拍价依 capability；Final 不等于完成结算，提前结算受服务端策略控制 |
| 结算/Recovery | 先选 settled 目录，再按匹配 UUID/world/期号读取报告，确认交给服务端重验事实 |
| 邮件/Delivery | 审查快照与历史决定分开；重试只表示重新排队，未知 Send 不可凭裸引用确认成功 |
| Users/凭据 | secret 只在授权录入或登录 key 一次性响应中短暂存在，不写浏览器持久化、日志或普通列表 |
| 审计/Realtime | 审计只读；最近事件仅元数据，replay 需显式确认，不伪造未冻结外部 sink 成功 |
| Dashboard | 每次读取一份安全快照，未采集与不可用显式区分，命令接受不表示任务完成 |

`mail_disposition_conflict` 隔离只允许核对后标失败，禁止成功确认和重试；关联邮件任务复用相同限制。
凭据更改提示 Worker 重启；A44 只保留当前页的分范围认证观测，不能证明写权限或改变凭据状态。
邮件 Cookie 导入可填写 `player_name`（可选、精确完整游戏名称，非数字 UID）；它只表示预期值，
forum Cookie 不接受该字段。表单与列表均区分预期名称和远端证明，现有可用性/余额检查不是身份证明。
目前可信自身身份页面协议尚未闭合，配置预期名称仍返回 `identity_unproven`，不会自动内部免寄。
旧记录保持空绑定；保留原凭据供未知投递对账，不能用改标签或删除重导入代替安全补绑定流程。
凭据录入不进入未决命令集合，Cookie/密码仅留原表单内存，不新增到浏览器存储。
导入一旦结果未知，锁定 kind/world/label/player_name/secret 原值与 UUID；输入处理器与控件同时守卫，
只能原样重试，后续认证、限流或业务拒绝（包括冲突）不解除累计未知，明确成功才复位。
离开路由或登出清理表单，不自动重放；既有启停/改名的冻结参数与 pending owner 不变。
接口和能力细节见[Admin 契约](../../docs/contracts/routes.md#admin)。

既有 delivery 列表兼容独立 `lot_watch` 来源并使用五语言类型标签，不新增页面、列、渠道或动作。
只为 `lot_watch_invalidated` 的明确 failed/无 sending/无租约/无外部引用/零 COD 投影接受 preflight 时间链；
此类行不提供重试入口。普通失败与 manual_reconcile 仍保持原严格时间链与 Sent 证明规则，服务器独立复核来源资格。

## 规则及运行配置

规则按 locale 读取与编辑，校验/发布期间禁止语言切换，迟到结果复核目标语言及草稿。
只有最近校验过的草稿可携带 revision fence 发布；冲突后重新读取，不能覆盖新版本。

Runtime 编辑发送非敏感 JSON 候选，不在浏览器直接读写 TOML。保存使用 fingerprint 与稳定命令；
六组 operations 的 managed 默认关闭，未接管候选不代表当前生效配置。
经济模板/排期只影响后续拍卖，保存后重启受影响进程，不热加载。
Runtime world 开关只控制 Worker 候选，数据库业务世界由独立版本化命令管理，不能混作业务已关闭。
详细生效范围见[配置手册](../../ops/config/README.md#runtime-共享与重启)。

## 认证与首屏边界

Basic Auth 只用于登录交换，后续使用独立会话/CSRF Cookie；站内 return 地址受限，登录后 replace 返回。
业务 401 触发统一会话复核，只有会话接口确认未认证才回登录；网络或解码失败保留可重试错误。
会话 touch 按服务端时间重新安排，绝对过期仍由服务端决定；Admin 重启后旧会话失效。

弹窗焦点循环、背景滚动与关闭后焦点恢复由共享层处理；真正换页才聚焦主内容。
页面任务和事件监听跟随宿主卸载，不能成为跨身份全局任务。
Gloo 只启用 HTTP/JSON，不继承 Public 的 WebSocket feature；可见性等待使用 `std::future::poll_fn`，
不另声明直接 futures-util 依赖，原有唤醒、分片计时和卸载清理逻辑不变。
首屏超过 15 秒仅提示慢加载，真实 WASM panic 才标记运行异常。服务端数据密钥不进入浏览器。

## 维护与验证

```bash
cargo check --locked -p auction-web-admin --target wasm32-unknown-unknown
```

仅静态编译；默认门禁与行为测试范围见[开发指南](../../docs/development.md)，运行证据见[验证状态](../../docs/status.md)。
路由、交互或字段变化须同步所属设计与契约，编译通过不等于浏览器验收。
