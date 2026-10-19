# 公开端浏览器界面

[文档导航](docs/README.md) · [Public 设计](crates/auction-web-public/DESIGN.md) · [架构](docs/design/architecture.md) · [契约](docs/contracts/README.md)

Dioxus 纯客户端渲染的公开端 SPA，拥有公开读取、竞价交互、会话与实时恢复。只消费稳定 HTTP/实时契约和共享浏览器能力，
不依赖 Admin UI、存储模型、配置或服务端密钥；页面结构、视觉与交互详见 [Public 设计](crates/auction-web-public/DESIGN.md)。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/app/surface](crates/auction-web-public/src/app/surface) | 路由、外壳、看板、目录与查询状态 |
| [src/app/auction](crates/auction-web-public/src/app/auction) | 拍卖读取、普通竞价与代理交互 |
| [src/app/content](crates/auction-web-public/src/app/content)、[src/app/identity](crates/auction-web-public/src/app/identity) | 规则、论坛、账户及认证页面 |
| [src/api](crates/auction-web-public/src/api) | 同源请求、时钟采样与严格 DTO 解码 |
| [assets](crates/auction-web-public/assets)、[i18n](crates/auction-web-public/i18n) | Public 样式与五语言译文 |

## 职责与实现边界

15 条浏览器路径由 Route、[后端入口清单](crates/auction-http/src/assets/routes.rs)和[路由契约](docs/contracts/routes.md)共同维护。
`main.rs` 为薄启动入口，`app/` 与 `api/` 按同一业务 owner 分组；`auction/watches/` 拥有整场装备标记读取、原生命令按钮
及账号隔离缓存（对应传输在 `api/auction/watches.rs`），`app/identity/watch_preference.rs` 与
`api/identity/watch_preference.rs` 拥有账号竞价警告渠道表单与严格解码，账户页以用户 UUID 隔离宿主。
`surface/board/refresh.rs` 拥有资源失效与合并读取，`auction/commands/` 按普通出价、代理创建/降额、确认及未知结果
重试拆分，`surface/pagination/` 统一各列表的数字分页、正整数数量与译文（`preferences.rs` 拥有 Public 本地数量偏好）。

论坛页先读取独立同步开关：明确关闭不挂载帖子轮询，未知状态不能当关闭；启用后只展示同步摘要、真实处理楼层和
更新时间，正文、版本与诊断细节归 Admin，公开页不提供手动重试按钮，失败后暂停轮询并在重新进入页面时重读。
首页不读取论坛状态，`/status` 只作静态说明。目录、历史和账户的筛选/分页通过 URL 恢复，响应须与当前选择身份对齐；
请求按所选数量交给后端分页，不在浏览器下载全量后切片（`api/decoder/` 仅为 `page_size` 接受完整正 `i64`，
金额与版本仍保留既有安全整数校验），具体数量与展开规则见
[Public 设计](crates/auction-web-public/DESIGN.md#5-表格与信息密度)。

## 国际化

业务文案统一通过 `t!` 读取本 crate 的 `i18n/*.ftl`，默认 en、编译期内嵌；消息 ID 不含点号，新键同步五种语言，
缺键会 panic。`hv.lang` 和 `<html lang>` 由共享层维护；规则请求把 locale 作为资源依赖，迟到响应不能覆盖当前语言，
正文用安全 Markdown 展示。

## 构建与资源边界

```bash
bash ci/release/web-bundle.sh public
```

该任务使用 `dx bundle`，复制到 `target/dx/build/auction-web-public/web/public/`，供 Public 的显式 `web-embed` 制品内嵌。
无 SSR/hydration、独立 JavaScript 业务代码或第二套前端构建链；后端保留 `allow_missing`：无产物可编译，已登记页面
返回 503，未知路径保持 404（见[开发指南](docs/guide/development.md#浏览器与发布构建)）。

## 权威数据与命令交互

目录、lot、规则、账户、价格历史等均读取同源 API，不使用会被误认成权威数据的示例行；服务端 capability 决定动作，
空装备链接不生成伪造 URL，完整游戏链接只消费受限 DTO。补偿流水复用目录的完整 ID/key 链接校验，
不能把带 10 位 key 的合法链接当作无效响应；字段可省略，提供时不接受空串或 keyless URL，见
[读取契约](docs/contracts/routes.md#公开读取与命令结果)。普通出价使用整数 K 输入，代理支持约定的精确缩写，
新命令最高 1B，服务端仍是金额、资格和状态的最终权威；提交前确认，失败保留冻结输入与命令 ID，结果未知只能重试
原请求。命令宿主按 lot/用户 UUID 隔离，切换目标或身份销毁旧状态，成功结果不被后台快照刷新覆盖。

补偿流水隐藏期初/期末占位条目，整期权威结算等式固定在每页表格跨全列的末行。历史成交价默认只显示居中搜索区，
提交筛选后才读取和展示成交列表，带筛选 URL 可直接恢复结果；布局与分页规则见[Public 设计](crates/auction-web-public/DESIGN.md#5-表格与信息密度)。

装备标记：整场通过 `GET /api/v1/me/lot-watches?auction_id=<uuid>` 读取完整集合（上限 10000 条，超限失败而非截断），
`PUT /api/v1/lots/{lot_id}/watch` 使用冻结的 `PublicSetLotWatchRequest`，`channel=mm` 保留为兼容占位。按稳定 user UUID +
auction UUID 隔离 `localStorage` 非敏感缓存（无登录不做本地匿名标记），缓存只作带未同步说明的展示，加载中或失败时
不能用旧版本修改；取消的 false 墓碑原样保存，命令成功后随既有刷新代数重读服务器集合，不从代理 GET 推导标记。
未知结果保留完整原命令供重试，409 必须由用户刷新、检查状态再修改（见[标记设计](crates/auction-web-public/DESIGN.md#61-装备标记与超价提醒)）。

账号渠道：`/me#outbid-notifications` 顶部提供统一的 MM/论坛 PM 单选与显式保存，
`GET/PUT /api/v1/me/lot-watch-preference` 消费 `PublicLotWatchPreferenceResponse` 与
`PublicSetLotWatchPreferenceRequest`，拒绝未知/缺失 data 字段、身份不符及非正 i64 版本，整数不经 JS 浮点转换。
权威 GET 前禁保存，不用本地默认值冒充服务器结果；PUT 冻结完整 payload、command_id 与 CAS 版本，成功或幂等重放后
GET 当前状态，离开/重载先 GET、不自动重放，待定命令不写入 localStorage，身份变化或登出销毁旧命令。
偏好只影响已标记装备的后续反超警告（每领先周期一次，不影响结算/COD），保存成功不保证可送达；论坛 PM 需已绑定的
论坛账号，不可投递不会自动改发 MM。当前世界选择只展示 enabled world，历史筛选仍可访问已关闭世界的 settled 事实；
倒计时通过 HTTP Date 估算服务端时钟，仅用于展示。

## 监听器所有权与实时恢复

已认证外壳订阅 discovery，拍卖页订阅相应单拍卖流，`/status` 不建连接。持久化游标、连续 sequence/version 与权威快照
决定恢复；`refetch_required` 保留高水位和现有 socket，不能用事件 payload 推导价格、领先者或代理权限（见
[实时契约](docs/contracts/events.md)）。普通竞价事件只失效相关已挂载资源，从首次通知起合并 100 毫秒，进行中的读取
先完成再补读未消费通知；生命周期、discovery 或缺口立即触发权威刷新。浏览器不因没有业务消息推断失联；
visibilitychange、popstate、socket 和回调随所属外壳卸载清理，游标不能因重建 owner 退回零。

## 依赖与首屏边界

HTTP/stream wire DTO 直接复用 `auction-contracts`，本地只持有查询、展示和交互状态；Gloo 在本 surface 显式启用
JSON/WebSocket，不能依赖 Admin 或合并构建补齐 feature。会话读取失败保留错误与重试入口，不能伪装匿名；
登录、注销及认证失效通过 `hv.public.session-revision` 仅广播随机修订，其他标签页据此重读会话；窗口聚焦及恢复可见时也重读。
随机修订与本地登录不预先清除身份，权威响应确认换号后才销毁旧宿主，同账号重登保留原未知命令及 ID。
竞价/代理确认与重试冻结原用户 UUID，标记、警告渠道及注销同样携带可选 `x-auction-expected-user-id`，由 HTTP 与本次 Cookie
身份核对，避免预检与提交之间换账号。401、`session_changed` 或本地显式注销先失效旧身份并卸载旧确认宿主。
登录 return 只接受单斜杠开头的站内路径。首屏 15 秒未就绪只提示慢加载，真实 panic 才显示失败。

## 维护与验证

```bash
cargo check --locked -p auction-web-public --target wasm32-unknown-unknown
```

仅静态编译；默认门禁与行为测试范围见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。
路由、交互或字段变化须同步所属设计与契约，编译通过不等于浏览器验收；未新增或运行行为测试。
