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
`app/identity/account/watches/` 拥有当期标记列表的只读组合、账号/世界隔离、完整目录读取与本地分页展示。
`surface/board/refresh.rs` 拥有资源失效与合并读取，`auction/commands/` 按普通出价、代理创建/降额、确认及未知结果
重试拆分，`surface/pagination/` 统一各列表的数字分页、正整数数量与译文（`preferences.rs` 拥有 Public 本地数量偏好）。

论坛页先读取独立同步开关：明确关闭不挂载帖子轮询，未知状态不能当关闭；启用后只展示同步摘要、真实处理楼层和
更新时间，正文、版本与诊断细节归 Admin，公开页不提供手动重试按钮，失败后暂停轮询并在重新进入页面时重读。
首页不读取论坛状态，`/status` 只作静态说明。目录、历史和账户的筛选/分页通过 URL 恢复，响应须与当前选择身份对齐；
请求按所选数量交给后端分页，不在浏览器下载全量后切片（`api/decoder/` 仅为 `page_size` 接受完整正 `i64`，
金额与版本仍保留既有安全整数校验），具体数量与展开规则见
[Public 设计](crates/auction-web-public/DESIGN.md#5-表格与信息密度)。
Public 查询同时订阅 Router 完整位置，普通链接与同路径 query 跳转均同步页头世界、页面读取与实时目标；
关闭世界只纠偏当前入口，不改写历史筛选或指定场次深链。分页用户操作使用 Push，服务端总数收缩后的页码纠偏使用
Replace，避免浏览器返回被无效页卡住；账户竞价/代理列表须同时匹配本次两个页码，重读期间不把旧行显示在新页码下。
竞价记录的世界目录与记录页分别保留请求身份，往期拍卖列表同时绑定世界、状态、页码和数量，指定历史拍卖绑定请求 UUID。
黑名单、补偿池批次、lot 竞价历史及账户列表先核对请求分页，再展示成功、空结果或错误；切换条件时旧响应不能展示为新条件的
结果，也不能触发新页码纠偏。同一查询的后台重读继续显示已完成快照。lot 详情按 UUID 重建宿主，避免 Router 复用组件时
沿用首次捕获的目标和命令状态。

目录顶部先显示“装备分组：全部／稀有／融合”，再显示装备类型筛选；当前、预览及历史某场目录共用。
分组与类型组合交给服务端分页，切换任一条件回第一页；`equipment_group=rare|fusion` 写入 URL，省略表示全部。
列表装备列仅显示装备名称，后续按“分组 → 平均 Roll”排列；分组列以文字显示稀有／融合，详情在装备事实中显示分组；
装备名保持原样式，论坛上色规则由服务端判定。
详情按“装备分组、分组值、标记按钮、分类、等级、平均 Roll”保持单行横排，标签和值同行；窄屏仅此信息栏可横向查看完整内容，
支持触控和键盘滚动，标记状态与同步门禁保持一致。
严格解码只接受服务端 `equipment_group=rare|fusion`，预览详情同时核对摘要与装备分组；类型计数取当前分组内总数，
目录快照身份包含分组，后台刷新、翻页和浏览器返回均保留并核对当前选择。

目录操作列由 `surface/catalog` 按权威拍卖状态控制；已结算场次同步移除列宽、表头及行单元格，
保留成交价格及装备分组等十二列；预览目录为十一列，未结算实时目录为十三列。各布局均分配完整的 100% 宽度。
`surface/board/data.rs` 按账号、世界、场次、阶段及筛选分页隔离目录快照；预览后台重读期间保留已对齐的
完整目录，新修订对齐后无提示替换，首次读取、上下文切换与明确失败继续提供原有反馈。

## 手机浏览器布局

Public 在 48rem 以下使用可展开导航；64rem 以下的触控设备横屏也使用同一布局。页头、内容与启动占位考虑
屏幕安全区，页面跟随动态视口高度并保留浏览器缩放。窄屏保留业务表的表头、百分比列宽和每一列，目录操作列内
按出价、代理、标记的原顺序纵排；筛选标签、会话名称及历史汇总允许换行，详情事实栏保持单行，触控动作提高到 44px 高度。
价格历史表单、登录与出价表单使用浏览器原生输入控件及回车提示；普通竞价保留整数数字键盘，代理上限使用可输入
K/M 缩写的文本键盘。弹窗、分页与输入字号由共享层适配，命令确认、幂等和权威读取仍沿用既有流程。

## 国际化

业务文案统一通过 `t!` 读取本 crate 的 `i18n/*.ftl`，默认 en、编译期内嵌；消息 ID 不含点号，新键同步五种语言，
缺键会 panic。`hv.lang` 和 `<html lang>` 由共享层维护；规则请求把 locale 同时作为资源依赖和结果身份，
语言改变时旧正文及旧错误立即退出展示，同语言后台刷新可保留已完成快照；正文用安全 Markdown 展示。

## 派生数据与解码

查询字段的 memo 借用状态，只复制实际需要的字段；账户和成交历史展示直接消费已拥有的行集合，不再次深克隆整页。
成功/错误信封从已拥有的 JSON 移出 `data`，成功值仍先执行原始字段及跨字段校验，再规范化兼容整数；
这只减少复制，不放宽响应接受边界。

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

补偿流水隐藏期初/期末占位条目，整期权威结算等式固定在每页表格跨全列的末行。历史成交价搜索区固定在页面顶部的独立全宽容器，不随下方结果表的列宽或字号适配变化；
页面只保留隐藏的可访问标题，移除可见大标题和说明。
提交筛选后才读取和展示成交列表，带筛选 URL 可直接恢复结果。名称按服务端相同的空白/逗号拆词、8 词、每词 128 字节和总计
1024 字节边界校验，离开字段或提交时显示五语字段错误，输入时清除错误。年月未改变时保留 URL 原有日期端点，单一清空
动作移除已应用条件并收起结果区；成交行在装备名旁显示原分类标的号，期数、等级和精确成交金额仍取权威 DTO。
名称片段以字面包含且全部命中，例如 `Leg Sav Pow Hel Bal` 可匹配各片段所在的完整装备名。
名称 URL 的非法值保留为可修正的字段错误并阻止请求，纯分隔符按空词集合处理，避免丢弃条件后扩大查询。
金额支持 C 整数、K 最多三位小数、M 最多六位小数，使用 checked 整数运算统一为 `0..=9007199254740991` C；
URL 中任意合法整数 C 均无损恢复，非法金额或倒置区间保留字段错误并阻止查询。`history_sort` 保存时间/价格四种全结果排序，
变更排序回第一页；`history_group` 仅控制本页按装备名、买家或卖家分组，默认按装备名分组，切换分组不重取成交页。
每组以独立结果框显示名称和真实本页笔数，可展开收起，框内保留既有九列表格；统计仍覆盖全部匹配成交，精确中位数优先使用可选 `median_exact`。
买卖家名称提供完整玩家名的快捷筛选并保留其他条件，未知卖家占位不生成筛选动作；清空同时恢复默认排序与装备名分组。
展示控件与本页分组由[成交历史展示模块](crates/auction-web-public/src/app/auction/reads/history/presentation.rs)拥有。交互参考
[AmyBotV2-web](https://github.com/anon962/AmyBotV2-web/tree/385c546bee177d945f0fae64041f936cd415a530)
（MIT，2024 anon962），由本项目 Rust 独立实现，构建输入仍仅依赖本仓库。
布局与分页规则见[Public 设计](crates/auction-web-public/DESIGN.md#5-表格与信息密度)。

装备标记：整场通过 `GET /api/v1/me/lot-watches?auction_id=<uuid>` 读取完整集合（上限 10000 条，超限失败而非截断），
`PUT /api/v1/lots/{lot_id}/watch` 使用冻结的 `PublicSetLotWatchRequest`，`channel=mm` 保留为兼容占位。按稳定 user UUID +
auction UUID 隔离 `localStorage` 非敏感缓存（无登录不做本地匿名标记），缓存仅用于展示；同步过程不显示加载或缓存提示，加载中或失败时
不能用旧版本修改；取消的 false 墓碑原样保存，命令成功后随既有刷新代数重读服务器集合，不从代理 GET 推导标记。
整场读取只构造一次按 lot UUID 索引的内存快照，权威结果与展示缓存通过 `Rc` 共享，避免每行线性扫描与整页深复制；
本地持久化仍保存原 DTO，版本校验及缓存不能提交命令的边界不变。
未知结果保留完整原命令供重试，409 必须由用户刷新、检查状态再修改（见[标记设计](crates/auction-web-public/DESIGN.md#61-装备标记与超价提醒)）。

`/me` 的[当期标记列表](crates/auction-web-public/src/app/identity/account/watches)按各启用世界读取当前场次，包含 preview。
现有完整标记集合与当前公开目录求交，显示真实装备名、分类编号、世界场次、标的状态及详情链接；撤回等非公开标的沿用目录可见性。
目录按 200 条完整分页，检查场次、公开修订、每页总数与重复标的，最后复核当前场次和完整标记版本；
既有 offset 10000 上限下最多读取 10200 条目录，超界、变化或读取失败显示错误与重试，不展示部分结果。
标记有效集合为空时跳过目录读取；最终匹配结果按真实总数每页 10 条，分页独立于竞价/代理页码。
按账号 UUID、世界与场次隔离；后台同步保留同账号/世界的已完成结果及分页，新结果返回后替换，新场次重建分页，
身份变化立即隐藏。列表不订阅普通竞价通知，会话续期不触发目录重读，不从本地标记缓存补装备数据。

账号渠道：`/me#outbid-notifications` 顶部提供统一的 MM/论坛 PM 单选与显式保存，
`GET/PUT /api/v1/me/lot-watch-preference` 消费 `PublicLotWatchPreferenceResponse` 与
`PublicSetLotWatchPreferenceRequest`，拒绝未知/缺失 data 字段、身份不符及非正 i64 版本，整数不经 JS 浮点转换。
权威 GET 前禁保存，不用本地默认值冒充服务器结果；PUT 冻结完整 payload、command_id 与 CAS 版本，成功或幂等重放后
GET 当前状态，离开/重载先 GET、不自动重放，待定命令不写入 localStorage，身份变化或登出销毁旧命令。
偏好只影响已标记装备的后续反超警告（每领先周期一次，不影响结算/COD），保存成功不保证可送达；论坛 PM 需已绑定的
论坛账号，不可投递不会自动改发 MM。当前世界选择只提供 enabled world；历史 URL 可显示已关闭世界的只读选中项，
历史筛选仍可访问已关闭世界的 settled 事实；
倒计时通过 HTTP Date 估算服务端时钟，仅用于展示。

## 监听器所有权与实时恢复

已认证外壳只为权威当前 `live`/`final` 场次建立单拍卖订阅；`/status` 是静态说明，不额外建立订阅。
持久化游标、连续 sequence/version 与权威快照决定恢复；`refetch_required` 保留高水位和现有 socket，
不能用事件 payload 推导价格、领先者或代理权限（见[实时契约](docs/contracts/events.md)）。
普通竞价事件只失效相关已挂载资源，从首次通知起合并 100 毫秒，进行中的读取
先完成再补读未消费通知；生命周期、discovery 或缺口立即触发权威刷新。浏览器不因没有业务消息推断失联；
visibilitychange、popstate、socket 和回调随所属外壳卸载清理，游标不能因重建 owner 退回零。
隐藏等待每个 future 仅登记一个 Waker 槽位，重复 poll 只更新唤醒目标；取消立即移除自己的登记，不等恢复可见再清理。
可见唤醒后若再次隐藏，仍可重新登记；其余轮询或退避等待者不受单个任务取消影响。
同一世界权威快照重读期间保留最近已确认的实时目标；缺口后的后续健康页可恢复 Live，永久错误或重试耗尽的 Manual 不由迟到页覆盖。

## 依赖与首屏边界

HTTP/stream wire DTO 直接复用 `auction-contracts`，本地只持有查询、展示和交互状态；Gloo 在本 surface 显式启用
JSON/WebSocket，不能依赖 Admin 或合并构建补齐 feature。会话读取失败保留错误与重试入口，不能伪装匿名；
登录、注销及认证失效通过 `hv.public.session-revision` 仅广播随机修订，其他标签页据此重读会话；窗口聚焦及恢复可见时也重读。
随机修订与本地登录不预先清除身份，权威响应确认换号后才销毁旧宿主，同账号重登保留原未知命令及 ID。
竞价/代理确认与重试冻结原用户 UUID，标记、警告渠道及注销同样携带可选 `x-auction-expected-user-id`，由 HTTP 与本次 Cookie
身份核对，避免预检与提交之间换账号。401、`session_changed` 或本地显式注销先失效旧身份并卸载旧确认宿主。
登录 return 只接受单斜杠开头的站内路径。首屏 15 秒未就绪只提示慢加载，真实 panic 才显示失败。

浏览器只在已认证且当前拍卖为 `live`/`final` 时建立拍卖实时连接；预览、准备开拍、结算中或无当前场次均停用，
预览徽章显示“预览”。当前场次每 60 秒后台复核及恢复可见补读继续生效，确认进入竞价期后自动连接。
“立即刷新”按钮先显示忙碌反馈，再执行整页重载；预览期仍可手动刷新，自动数据同步不重载页面。

## 维护与验证

```bash
cargo check --locked -p auction-web-public --target wasm32-unknown-unknown
```

仅静态编译；默认门禁与行为测试范围见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。
路由、交互或字段变化须同步所属设计与契约，编译通过不等于浏览器验收；未新增或运行行为测试。
