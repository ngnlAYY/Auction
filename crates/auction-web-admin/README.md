# 管理端浏览器界面

Dioxus 客户端渲染的管理端 SPA。路由集合按旧版 Auction-BF 的
`manifests/admin.routes.json` 冻结，共 21 条，并与
`auction-http/src/assets/routes.rs` 的后端入口清单同步维护。

- `src/main.rs`：薄启动入口。
- `src/api/`：按 `auction/`、`content/`、`fulfillment/`、`identity/`、`operations/` 分组的
  安全投影、会话读取和命令请求。
- `src/app/`：按相同 owner 分组的页面组件；拍卖列表在 `auction/auctions/` 内拆分创建、表格、
  生命周期动作 `actions/{row,dialog,model}.rs` 和共享映射，结算页在 `auction/settlement/` 内拆分拍卖选择、结算报告、recovery 交互和
  辅助映射，用户页在 `identity/users/` 内拆分创建/key、筛选表格、用户行和状态对话框，凭据页在
  `identity/credentials/` 内拆分页面编排、安全录入、凭据表格、确认对话框和安全标签辅助，Dashboard 在
  `operations/dashboard/` 内拆分单快照页面编排、旧版指标内容和邮件控制；lot 页在 `auction/lots/` 内拆分
  页面加载、表格编排、单行更新、代理历史和展示辅助；邮件入站页在 `fulfillment/mail/` 内拆分页面筛选、
  轮询动作、入站表格、详情投影和状态标签；`shared/` 只保存本 surface
  内真正通用的基础组件。`surface/mod.rs` 只保留 surface 装配和资源初始化，`surface/router.rs` 持有
  路由与页面入口包装器，`surface/auth/` 按会话状态编排、return 路径、重试/刷新、登录表单和认证后外壳拆分，
  分别由 `auth/{mod,redirect,session,login,shell}.rs` 持有，`surface/shared.rs` 持有跨页面状态面板、
  错误重试和时间格式化；客户端未知路由也位于 `AdminShell` layout 内，不能绕过会话加载、错误或
  登录外壳直接渲染管理端 404；realtime 页面在
  `operations/realtime/` 内拆分页面状态/replay、筛选器、lag 表/分页和 failure 表/分页；delivery outbox
  在 `fulfillment/deliveries/` 内拆分页面编排、状态筛选、列表/分页、动作对话框、共享状态模型和标签辅助；
  规则页面在 `content/rules/` 内拆分页面加载、编辑 workspace、Markdown 预览、发布状态模型和字段辅助。
- `assets/admin.css`：管理端 surface 样式。设计变量来自 `auction-web-shared`。
- `i18n/*.ftl`：5 种语言的 Fluent 译文（`en`、`zh-CN`、`zh-TW`、`ja`、`ko`），默认简体中文。

## 国际化

所有用户可见文案都经 `t!` 从 `i18n/*.ftl` 取用，组件里不得硬编码字符串。译文以
`include_str!` 编译期内嵌，首屏不额外请求语言资源。

Fluent 消息 ID 不允许包含点号，旧系统的 `nav.home` 一类键改为 `nav-home`。

`t!` 在键缺失时 **panic**，因此新增文案必须同步更新全部 5 个 `*.ftl`；缺一种语言就会让该
语言下的页面崩溃。语言集合、偏好持久化和 `<html lang>` 同步由 `auction-web-shared` 提供，
本 crate 只拥有自己的译文。

构建：

```bash
just web-build-admin   # 通过 dx bundle 复制到稳定嵌入目录
just web-serve-admin   # 本地开发监听
```

`just web-build-admin` 内部先执行 Web 构建，再将产物复制到
`target/dx/build/auction-web-admin/web/public/`，供 Admin 装配根的
`rust-embed` 使用；浏览器标题由本 crate 的
`Dioxus.toml` 固定为旧版 `Auction 管理站点`，静态入口通过 `/favicon.ico` 引用共享品牌图标；产物由 Admin 装配根独占的
`RustEmbed` 类型内嵌，再交给 `auction-http` 提供。依赖图不引用 Public 产物；未知路径保持 404，
不获得 Admin SPA 入口。

管理端与公开端是不同 surface：本 crate 独立拥有自己的组件树和路由树，不复用公开端组件，
公开端也不得依赖本 crate。拍卖列表已接入 PostgreSQL 只读安全投影、旧版 world 筛选、时间线和分页，
生命周期按钮会遵守后端返回的 capability，并使用状态版本 fence、确认对话框和动作原因；Live 状态下
“进入最终时刻”与可选的“提前结算”并列，Final 状态仅保留已启用的提前结算。前者把较晚的 lot 截止
时间收紧到默认 15 分钟最终窗口并继续允许软截止竞价，后者要求理由且由默认关闭的服务端 capability
控制，浏览器不直接推断或写入结算事实；lot 页面也已接入同一安全投影和真实分页；创建拍卖命令和管理端创建表单
已接入 PostgreSQL 完整档。
拍卖筛选栏恢复旧版的轻量横向布局，手工录入恢复五列/窄屏单列表单，创建弹窗字段改为稳定的纵向
布局，避免通用操作栏挤压时间和 JSON 输入；手工录入成功后再次修改任一来源或命令字段会清除旧结果，
避免把上一次提交误认为当前输入的结果；
draft pending lot 还提供起拍价修改与撤回按钮；动作使用确认对话框，携带按同一输入重试时复用的 `command_id`、快照版本和 CSRF，
成功后重新读取权威 lot 页面，并展示软截止信息、设备属性悬浮详情、金额/时间格式化和代理历史关闭/重试入口；
代理历史表补齐 caption、列标题与窄屏字段卡片，
不在浏览器端复制 lot 状态机。
管理端外壳恢复旧版的扁平导航顺序与 `/cookies` 单一凭据入口；窄屏时导航折叠为带明确展开状态的
顶部菜单，主内容不再被固定宽侧栏挤压；不在导航中显示重复别名和
`/status` 状态入口；登录页也恢复旧版无卡片边框的窄表单布局，并移除会与卡片外边距叠加的根节点
`100vh`，无错误文案时桌面和常见移动 viewport 不产生无意义页面滚动；旧别名仍可直接访问并无历史记录跳转到 canonical 地址。
拍卖与 lot 表还保留旧版的状态徽章、动作组和窄屏 `data-label` 字段布局，避免宽表在移动端丢失字段语义。
delivery、凭据、Users、改名、realtime、邮件入站、审计和结算/Recovery 表也使用状态徽章、动作组和窄屏字段标签；
已完成旧投影收口的列表在成功空页仍保留表头，空态提示作为独立状态显示，页面不依赖隐藏表头
表达字段含义。
改名回滚确认框、审计筛选、拍卖开放就绪提示和金额字段使用独立的旧版对齐样式；改名/lot 确认框插入
表格时不会再撑开空白行，窄屏审计筛选会自动收缩为两列或单列。列表中的长标识、名称和详情统一
保持单行省略，鼠标悬浮或键盘聚焦可查看完整值，不以自然换行撑高记录。
delivery 页面已接入 PostgreSQL 安全投影、状态筛选、数字分页，以及 release/retry/人工对账动作；
release/retry 使用确认对话框，manual_reconcile 支持 retry、succeeded（外部引用）和 failed（失败代码）
三种决策。操作员取消人工对账即显式放弃本次草稿和命令状态，普通提交失败仍保留输入与稳定
命令 ID 供重试。命令携带 `command_id`、CSRF 和 fencing version，服务端仍负责最终校验，浏览器不推断外部回执。
投递状态筛选恢复旧版的轻量横向控件，窄屏时切换为全宽纵向控件；列表和外部回执状态展示保持不变。
Realtime 运维页面已接入 lag/failure 安全投影、stream/consumer/code/resolved 筛选、分页、
空结果状态和旧版风格的可展开高级筛选区；已解决 failure 不允许再次发起 replay。replay 使用
带遮罩的确认对话框，提交期间锁定输入并保留失败反馈；公开 payload 由操作员输入，服务端负责
schema、幂等、审计和事件关系校验。
`/status` 沿用旧版静态产品说明，不创建 live/ready 请求；登录外壳仍单独读取 `/health/ready`，
其 503 按未就绪显示，不与 Dashboard 运维投影混用。
Realtime 页面恢复旧版标题、连接状态与高级筛选的同一相对定位壳层；宽屏高级筛选作为右上角
弹层呈现，窄屏自动回到文档流，lag/failure 投影、分页、replay 确认和外部广播 sink 边界保持不变。
审计页面已接入追加式安全记录查询、操作者/实体/来源/结果筛选、UTC 时间范围、有界数字分页和
加载失败重试；筛选草稿只在提交表单时触发查询，不会在每次按键时制造请求风暴；metadata 只作为
JSON 文本展示，不提供历史编辑。
审计筛选把操作者 ID、实体 `类型:ID`、动作和请求 ID 作为常用精确条件，把操作者类型、来源、结果和
UTC 时间收进可展开的高级条件，并提供一键清除；宽屏四列、窄屏两列或单列。审计记录仍为只读安全
投影；操作者和实体分别合并为 `ID(类型)` 的单行值，长 ID/metadata 可截断查看且不再把所有 metadata
误标为错误色。记录表在宽屏使用固定列宽和语义状态标签，窄屏按列标题切换为字段卡片。
结算页面已接入已结算拍卖选择、结算 lot 金额表、资金池摘要、recovery 记录和旧版同语义的确认对话框；lot 表
直接展示服务端投影的持有比例、调整比例和挂牌价，并以明确文字及正负号区分折扣/惩罚，不靠颜色传达含义；
recovery 确认携带报告中的 state version、CSRF 和按同一输入重试复用的 command ID，服务端仍负责证据重校验、幂等和
事务结果，页面不在浏览器端计算结算金额或改变 recovery 状态。
Outbox 页面已接入 realtime failure、consumer lag 和安全 replay 投影；未冻结的外部广播 sink 不会被
页面伪造成已发送或已成功。
规则页面已接入按 locale 读取、语言标签、动态新增 locale、版本事实、候选规范化校验、
轻量 Markdown 预览和带 revision fence 的发布表单；只有最近一次已校验的草稿才允许发布，
发布会复用稳定的 command ID，并对版本冲突提供重新加载反馈。浏览器只发送版本化 DTO，不持有
数据库或凭据状态。
规则编辑器恢复旧版的事实栏间距、36rem Markdown 编辑区、48rem 预览上限和窄屏单列切换；
校验、发布门槛与 revision fence 不变。
运行配置页面已接入非敏感 JSON 文档读取、对象解析提示、服务端候选规范化校验、fingerprint
版本 fence、稳定 command ID、可见的加载失败重试、冲突反馈和重启提示；文档已恢复旧版 main/isekai、
发件人 allowlist、装备过滤和 14 个通知模板，并用启用 world/装备过滤事实卡恢复旧页面摘要。更新
不会被页面误报为热加载，业务字段保存后提示重启受影响的 Admin 与 Worker。
配置编辑器恢复旧版的全宽无圆角代码区和 34rem 最小高度，页面标题、说明和 fingerprint 状态保持
清晰分层；配置校验、保存和重启提示的契约不变。
邮件页面已接入 A16-A19 的轮询状态、入站安全摘要和暂停/恢复动作；动作携带 CSRF、当前
状态版本与命令 ID。页面首次打开会聚合全部已配置邮箱的入站记录，输入具体 mailbox 后再
读取该邮箱的轮询状态；入站列表和轮询状态读取失败都保留当前查询并提供原位重试；入站列表按旧页面支持 world、kind、status 筛选和 10 条分页
和安全元数据详情；页面不接收原始邮件正文、Cookie、轮询游标或外部凭据。A20 触发按钮已由
HTTP/Worker 链路接入，使用单个 command ID 唤醒全部已装配 poller，返回的是接受状态，不代表业务邮件处理完成。管理总览会展示全部 mailbox
的轮询状态，并提供逐邮箱暂停/恢复与全局立即轮询操作。管理命令统一显示忙碌、禁用重复提交并以
成功/失败语义反馈；全部 Admin API 会先解析稳定错误 code，再通过五语言 Fluent 映射 stale version、
command conflict、权限、限流和依赖错误，未知 code 才使用兜底，不展示服务端原始错误。左侧导航恢复旧版扁平顺序，`/status` 等运维入口仍可通过直接 URL 访问，旧别名
会无历史记录跳转到 canonical 地址。
用户页面已接入 A28/A31 的安全用户列表、创建、改名、筛选分页、黑名单历史/次数/创建时间展示、论坛账号 ID 修改、key 撤销/重签和黑名单
操作；创建用户使用对话框，自动生成和重签的登录 key 使用独立的一次性展示框；论坛账号 ID、key 和黑名单动作使用确认对话框并要求黑名单原因；A30 回滚命令已接入 HTTP/API owner，
改名历史页面提供 A29 的名称筛选表格和 A30 回滚按钮；名称变化沿用旧请求时机立即刷新第一页，
不增加 user ID 查询控件。
页面只持有
安全用户投影，生成 key 仅在创建/重签成功响应中短暂展示，不保存数据库状态或外部凭据。
Users 页恢复旧版的标题与创建入口顶栏，创建按钮不再单独占用一行；窄屏时顶栏改为纵向排列，
创建对话框、一次性 key 展示和命令幂等边界保持不变。
凭据页面已接入 A32 的安全列表、固定分页、导入、对话框改名、启停和带确认的删除操作；列表按旧版不提供额外筛选栏；
secret 只通过当前表单请求提交，不会进入响应、持久化状态或页面回显；请求失败时保留当前表单以便用同一 command ID 重试。该页面仅在 PostgreSQL 完整档并配置独立
`credential_data_key` 时可用，修改成功后提示 Worker 重新加载凭据池。
凭据表还恢复旧页面实际合成的 last checked、last used、imported by 三列：分别使用现有
`rotated_at`、明确空值和当前 Admin session actor；它没有虚构后端使用记录，也没有扩展 secret DTO，
state version 只参与写命令 fence，不扩张旧版八列表格。
管理总览每次只请求一份完整安全 dashboard snapshot，并提供显式刷新动作；页面按旧版顺序展示 health/process、Worker、
tracing、实时 lag/failure、MoogleMail 轮询、凭据 availability、delivery、fulfillment、recovery
和 alerting 水位；拍卖、投递列表和邮件入站不再创建额外 Dashboard 请求。页面恢复旧版扁平指标卡、
连续卡片流、统一边框几何与邮件动作区，并把拍卖、邮件、投递和 realtime outbox 接成仅导航、不附加
数据请求的安全快捷入口；快照没有采集的 outbox retry/claimed/attempts 明确显示“未采集”，不再与真实空值混淆。
Dashboard section 与嵌套 data 使用旧 v1 严格字段闭集；扩展凭据 scope 和内部诊断不会原地进入旧
响应。MoogleMail `main`/`isekai` availability 仍按旧语义分别呈现，避免其他凭据掩盖邮件 world
缺少 active 凭据；配置 `dashboard_mailboxes` 时服务端会核对所有已配置 mailbox 是否都有轮询器。
邮件页的筛选栏、邮箱加载控件和轮询状态卡保留旧版的轻量横向间距；窄屏时自动切换为纵向表单，
不会让邮箱输入或闭集筛选压缩表格字段。
凭据页的导入区也恢复旧版的 32rem 纵向表单和页面标题顺序；列表不增加旧版不存在的筛选栏，
secret、状态动作和 PostgreSQL 完整档边界不变。
页面提供逐 mailbox 的运维动作、显式的不可用状态、手动刷新和常用运维页面
快捷入口；总览不会用前端假数据填充尚未接入的能力。
Admin 登录页面已接入已有的 Basic Auth 登录交换、双 Cookie 会话读取、CSRF 注销和会话失效后的
安全回退；未认证访问深链接时会保存受限的站内 `return` 地址，登录成功后无历史记录返回原页面；
浏览器还会按服务端 idle expiry 触发会话 touch，失败时按间隔重试；绝对过期仍由服务端决定。
浏览器只保存 actor、CSRF token 和过期时间，不保存数据库或外部凭据。
登录页与已认证外壳还会读取 `/health/ready` 并提供屏幕阅读器可见的 checking/ready/degraded
状态提示；该提示保持视觉隐藏，也不改变 `/status` 的旧版静态说明职责。
所有 `aria-modal` 管理弹窗还由壳层提供初始 `autofocus`/首控件聚焦、Tab/Shift+Tab 焦点循环、
背景滚动锁定和关闭后触发控件焦点恢复，避免键盘焦点离开当前弹窗或在关闭后丢失。用户行与凭据行
的局部动作反馈按成功/失败分别使用 `status`/`alert`，并以行名称组成可访问标签和描述关系；认证后
真正换页会聚焦主内容，首屏和同路由刷新不会抢占当前控件。
管理命令必须走版本化 REST 契约，不在浏览器端重建状态机；凭据与密钥材料不进入浏览器。
`src/api/` 的 HTTP/stream wire DTO 已直接复用 `auction-contracts`；本 crate 只保留分页展平、
Dashboard section 解码、表单与交互等浏览器展示状态。
