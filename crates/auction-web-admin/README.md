# 管理端浏览器界面

Dioxus 客户端渲染的管理端 SPA。路由集合按旧版 Auction-BF 的
`manifests/admin.routes.json` 冻结，共 21 条。

- `src/main.rs`：薄启动入口。
- `src/api/`：管理端会话登录/注销、拍卖、lot、结算/recovery、delivery/realtime/mail 安全投影、CSRF 会话读取和人工状态命令请求。
- `src/app/`：管理端路由表、外壳、管理总览与按职责拆分的页面组件。
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
just web-build-admin   # dx build -p auction-web-admin --platform web
just web-serve-admin   # 本地开发监听
```

产物输出到 `target/dx/build/auction-web-admin/web/public/`，由 `auction-http` 内嵌进管理端
二进制。公开端二进制不包含这些资源。

管理端与公开端是不同 surface：本 crate 独立拥有自己的组件树和路由树，不复用公开端组件，
公开端也不得依赖本 crate。拍卖列表已接入 PostgreSQL 只读安全投影、world/status 筛选、时间线和分页，
生命周期按钮会遵守后端返回的 capability，并使用状态版本 fence、确认对话框和取消原因；lot 页面也已接入同一安全投影和真实分页；创建拍卖命令和管理端创建表单
已接入 PostgreSQL 完整档。
拍卖筛选栏恢复旧版的轻量横向布局，手工录入恢复五列/窄屏单列表单，创建弹窗字段改为稳定的纵向
布局，避免通用操作栏挤压时间和 JSON 输入；
draft pending lot 还提供起拍价修改与撤回按钮；动作使用确认对话框，携带按同一输入重试时复用的 `command_id`、快照版本和 CSRF，
成功后重新读取权威 lot 页面，并展示软截止信息、设备属性悬浮详情、金额/时间格式化和代理历史关闭/重试入口，
不在浏览器端复制 lot 状态机。
管理端外壳恢复旧版的扁平导航顺序与 `/cookies` 单一凭据入口，不在导航中显示重复别名和
`/status` 状态入口；登录页也恢复旧版无卡片边框的窄表单布局，旧别名仍可直接访问并无历史记录跳转
到 canonical 地址。
拍卖与 lot 表还保留旧版的状态徽章、动作组和窄屏 `data-label` 字段布局，避免宽表在移动端丢失字段语义。
delivery、凭据、Users、改名、realtime、邮件入站、审计和结算/Recovery 表也使用状态徽章、动作组和窄屏字段标签；
页面不依赖隐藏表头来表达字段含义。
改名回滚确认框、审计筛选、拍卖开放就绪提示和金额字段使用独立的旧版对齐样式；改名/lot 确认框插入
表格时不会再撑开空白行，窄屏审计筛选会自动收缩为两列或单列。
delivery 页面已接入 PostgreSQL 安全投影、状态筛选、数字分页，以及 release/retry/人工对账动作；
release/retry 使用确认对话框，manual_reconcile 支持 retry、succeeded（外部引用）和 failed（失败代码）
三种决策。命令携带 `command_id`、CSRF 和 fencing version，服务端仍负责最终校验，浏览器不推断外部回执。
投递状态筛选恢复旧版的轻量横向控件，窄屏时切换为全宽纵向控件；列表和外部回执状态展示保持不变。
Realtime 运维页面已接入 lag/failure 安全投影、stream/consumer/code/resolved 筛选、分页、
空结果状态和旧版风格的可展开高级筛选区；已解决 failure 不允许再次发起 replay。replay 使用
带遮罩的确认对话框，提交期间锁定输入并保留失败反馈；公开 payload 由操作员输入，服务端负责
schema、幂等、审计和事件关系校验。
`/status` 直接展示 Admin `/health/live` 与 `/health/ready` 的存活/就绪状态；`ready` 的 503
按未就绪显示，并提供手动刷新，不与 realtime 运维投影混用。
Realtime 页面恢复旧版标题、连接状态与高级筛选的同一相对定位壳层；宽屏高级筛选作为右上角
弹层呈现，窄屏自动回到文档流，lag/failure 投影、分页、replay 确认和外部广播 sink 边界保持不变。
审计页面已接入追加式安全记录查询、操作者/实体/来源/结果筛选、UTC 时间范围、有界数字分页和
加载失败重试；metadata 只作为 JSON 文本展示，不提供历史编辑。
审计筛选恢复旧版按文本、枚举和日期字段分别分配宽度，并在 60rem/48rem 断点重排；审计记录仍为
只读安全投影。
结算页面已接入已结算拍卖选择、结算 lot 金额表、资金池摘要、recovery 记录和旧版同语义的确认对话框；
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
版本 fence、稳定 command ID、加载失败重试、冲突反馈和重启提示；更新不会被页面误报为热加载。
配置编辑器恢复旧版的全宽无圆角代码区和 34rem 最小高度，页面标题、说明和 fingerprint 状态保持
清晰分层；配置校验、保存和重启提示的契约不变。
邮件页面已接入 A16-A19 的轮询状态、入站安全摘要和暂停/恢复动作；动作携带 CSRF、当前
状态版本与命令 ID。页面首次打开会聚合全部已配置邮箱的入站记录，输入具体 mailbox 后再
读取该邮箱的轮询状态；入站列表支持 world、kind、status、decision、时间范围筛选、20 条分页
和安全元数据详情；页面不接收原始邮件正文、Cookie、轮询游标或外部凭据。A20 触发按钮已由
HTTP/Worker 链路接入，使用单个 command ID 唤醒全部已装配 poller，返回的是接受状态，不代表业务邮件处理完成。管理总览会展示全部 mailbox
的轮询状态，并提供逐邮箱暂停/恢复与全局立即轮询操作。管理命令统一显示忙碌、禁用重复提交并以
成功/失败语义反馈；左侧导航恢复旧版扁平顺序，`/status` 等运维入口仍可通过直接 URL 访问，旧别名
会无历史记录跳转到 canonical 地址。
用户页面已接入 A28/A31 的安全用户列表、创建、改名、筛选分页、黑名单历史/次数/创建时间展示、游戏身份修改、key 撤销/重签和黑名单
操作；创建用户使用对话框，自动生成和重签的登录 key 使用独立的一次性展示框；游戏身份、key 和黑名单动作使用旧版确认对话框并要求黑名单原因；A30 回滚命令已接入 HTTP/API owner，
改名历史页面提供 A29 的筛选表格和 A30 回滚按钮。
页面只持有
安全用户投影，生成 key 仅在创建/重签成功响应中短暂展示，不保存数据库状态或外部凭据。
Users 页恢复旧版的标题与创建入口顶栏，创建按钮不再单独占用一行；窄屏时顶栏改为纵向排列，
创建对话框、一次性 key 展示和命令幂等边界保持不变。
凭据页面已接入 A32 的安全列表、kind/world/status 筛选、分页、导入、对话框改名、启停和带确认的删除操作；
secret 只通过当前表单请求提交，不会进入响应、持久化状态或页面回显；请求失败时保留当前表单以便用同一 command ID 重试。该页面仅在 PostgreSQL 完整档并配置独立
`credential_data_key` 时可用，修改成功后提示 Worker 重新加载凭据池。
管理总览已接入完整的安全 dashboard snapshot：除拍卖、投递、实时 lag/failure 和 MoogleMail
全量轮询/入站摘要外，还展示 health/process、Worker、tracing、凭据 scope、fulfillment、recovery
和 alerting 水位；凭据状态还会显示最近变更后的 Worker 重启要求，无法验证时明确显示未知。
Dashboard 指标和运行时明细恢复旧版的扁平指标卡、连续卡片流和统一边框几何；安全摘要、轮询
操作和快捷入口仍保留。
同时按旧版语义分别显示 MoogleMail `main`/`isekai` 的 availability，避免 forum/shared 凭据掩盖
某个邮件 world 缺少 active 凭据；配置 `dashboard_mailboxes` 时 dashboard 还会按 Admin runtime
投影核对所有已配置 mailbox 是否都有轮询器。
邮件页的筛选栏、邮箱加载控件和轮询状态卡保留旧版的轻量横向间距；窄屏时自动切换为纵向表单，
不会让邮箱输入或闭集筛选压缩表格字段。
凭据页的导入区也恢复旧版的 32rem 纵向表单和页面标题顺序，kind/world/status 筛选保持轻量横向
布局，窄屏时切换为纵向控件；secret、状态动作和 PostgreSQL 完整档边界不变。
页面提供逐 mailbox 的运维动作、显式的不可用状态、手动刷新和常用运维页面
快捷入口；总览不会用前端假数据填充尚未接入的能力。
Admin 登录页面已接入已有的 Basic Auth 登录交换、双 Cookie 会话读取、CSRF 注销和会话失效后的
安全回退；未认证访问深链接时会保存受限的站内 `return` 地址，登录成功后无历史记录返回原页面；
浏览器还会按服务端 idle expiry 触发会话 touch，失败时按间隔重试；绝对过期仍由服务端决定。
浏览器只保存 actor、CSRF token 和过期时间，不保存数据库或外部凭据。
登录页与已认证外壳还会读取 `/health/ready` 并提供屏幕阅读器可见的 checking/ready/degraded
状态提示；该提示保持视觉隐藏，不替代 `/status` 页的完整 live/ready 展示。
所有 `aria-modal` 管理弹窗还由壳层提供 Tab/Shift+Tab 焦点循环、背景滚动锁定和关闭后触发控件
焦点恢复，避免键盘焦点离开当前弹窗或在关闭后丢失。
管理命令必须走版本化 REST 契约，不在浏览器端重建状态机；凭据与密钥材料不进入浏览器。
