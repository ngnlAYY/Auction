# auction-web-public

[docs/README.md](../../docs/README.md) · [crates/auction-web-public/DESIGN.md](DESIGN.md) · [docs/guide/player-guide.md](../../docs/guide/player-guide.md) · [docs/contracts/routes.md](../../docs/contracts/routes.md)

玩家使用的公开站点：Dioxus 0.7 客户端渲染（CSR）WASM 应用，构建产物由 `auction-public-api` 的 `web-embed` 构建内嵌并同源提供。页面标题固定为“Auction 公开站点”（发布门禁以此识别 payload）。

## 页面

| 路径 | 页面 |
| --- | --- |
| `/`、`/auction` | 拍卖看板：按世界切换，当前或历史场次的预览/正式目录、倒计时、实时连接状态，按装备分组（稀有/融合）、类型与等级筛选，可配置显示列，点击行或查看按钮打开拍品详情抽屉（含竞价历史、同名装备的历史成交价，以及登录后本人对该拍品的代理授权：上限、已用次数、模式与状态，打开抽屉时才读取），在行内或弹窗中出价、设置/下调代理、标记装备。顶栏的场次编号、状态与倒计时始终对应正文展示的场次（按 `auction_id` 查看历史场次时显示该场次）；领先者不能再出价，抽屉里的入口改为进入代理页签以下调已有授权 |
| `/auctions` | 历史拍卖列表：每场附拍品与出价记录入口，已结算场次另有履约进度入口（带场次与世界） |
| `/lots/{lot_code}` | 拍品直达链接：独立的拍品页已移除，拍品详情统一在各列表的页内抽屉里展示。参数是拍品 UUID 时（被超提醒里的链接）跳到看板上该拍品所在的目录页并打开详情抽屉；其余取值（旧书签、早期提醒里的 lot 代码）跳回拍卖看板，保留原地址中的世界与场次查询 |
| `/bidlog` | 出价日志：按世界、场次与拍品筛选，列出时间、编号、装备、竞价者与金额；操作列的查看按钮在页内打开只读的拍品详情抽屉。拍品筛选接受完整拍品 UUID 或不超过 32 位的字母数字编号，输入停顿后才生效，格式不符时就地提示、不发请求 |
| `/fulfillment` | 履约进度：按世界与已结算场次逐件列出编号、装备、卖家、买家、成交价与当前流程状态（流拍寄回、COD 付款与退回重寄、异世界交付、卖家货款与成交撤销），状态按语气分组着色并始终配文字；等待付款时附付款期限，发生过 COD 退回时附退回次数与上限。拍品筛选与出价日志同口径，每页最多 100 件（偏好或地址栏里 101–200 的每页数量按 100 请求（超过 200 的旧值回退默认数量），自定义输入超过 100 时就地提示 1–100 的整数范围）；操作列的查看按钮在页内打开只读的拍品详情抽屉。重读失败而展示旧快照时，表格上方提示该快照的确认时刻。状态与附带字段只来自服务端闭集，页面不推导流程 |
| `/history` | 成交价格历史：名称/日期/金额筛选、分组与中位数；装备名称不跳转，操作列的查看按钮在页内打开只读的拍品详情抽屉 |
| `/login` | 用登录 key 登录（支持 `return_to`） |
| `/me`、`/account` | 我的：当前标记、出价与代理，登录时长设置，竞价提醒渠道；标记、出价历史与代理三张列表都带查看按钮（页内打开只读的拍品详情抽屉），编号与名称不跳转；竞价提醒的 MM 与 PM 是两个独立开关，可都开或都关 |
| `/rules` | 规则文档（Markdown，五种语言） |
| `/about` | “关于”文档（Markdown，五种语言；尚未发布时显示空状态） |
| `/blacklist` | 黑名单 |
| `/compensation-pool`、`/compensation` | 补偿池余额、各场收支与逐件结算明细（含成交撤销冲正）；明细操作列的查看按钮在页内打开只读的拍品详情抽屉 |
| `/forum` | 论坛同步摘要 |
| `/status` | 服务说明 |

其他路径显示 404 页面。新增页面须同步修改 `crates/auction-web-public/src/surface/router/route.rs` 的 `Route`、后端 [crates/auction-http/src/assets/routes.rs](../auction-http/src/assets/routes.rs) 与 [docs/contracts/routes.md](../../docs/contracts/routes.md)。

## 数据与实时

- 业务请求走同源 `/api/v1/*`，外壳另读 `/health/ready`（只在启动门禁与页面恢复可见时读取一次，不做周期轮询；持续连接由实时 WS 与 API 失败反馈承担，WS 只承载拍卖事件，不等同于数据库/relay 就绪）；HTTP 经 `auction-web-shared` 的传输边界（默认 10 秒响应头与正文读取预算、可取消），响应按路由专属 decoder 解析为 `auction-contracts` DTO。
- 登录用户的看板与 lot 详情通过 `/api/v1/ws` WebSocket 接收实时事件（未登录时页头显示手动刷新模式）。连接建立后先用 `/api/v1/events` 补齐，序列跳号时重新补齐；断线按 1 秒起步、翻倍至 30 秒封顶并带随机抖动的退避重连，连续快速失败后降级为手动刷新；页面隐藏时暂停轮询与重连退避。普通竞价事件对相关资源做 100–250ms（基础 100ms 加每个窗口的随机抖动）合并的有界重读，生命周期变化或回放缺口触发权威快照刷新，不在前端推导竞价结果。
- 代理新建表单的初始模式取当前场次响应中该场冻结的 `proxy_default_mode`（Final 阶段不能新建 Slow 时为 Fast），玩家可以切换，提交总是显式携带模式。当前场次读取始终带 `include=proxy_default_mode` 显式请求该键（缺省响应保持旧页面的原字段集合），解码器要求它存在。按 `auction_id` 查看的未结算场次同样经 `/api/v1/auctions/current` 取得场次上下文；往期目录拼装的已结算场次没有该值，新建表单失败关闭。
- 竞价、代理与标记命令带稳定命令 ID；网络结果不确定时保留原负载供“原样重试”，不会生成新命令。标记、警告渠道与登录时长设置只有各自路由的明确 `409` 冲突码才解除未知状态；损坏响应、未识别的 `409` 与后续前置拒绝不能否定之前的提交。
- 会话 Cookie 由后端以 HttpOnly 下发；跨标签页通过 `storage` 事件触发权威身份重读，Cookie 写入共用原生 Web Lock。会话读取、Cookie 同步与身份变更的边界见 [crates/auction-web-public/DESIGN.md#会话](DESIGN.md#会话)。
- 列表响应绑定世界、场次、筛选、页码与每页数量等完整请求身份；切换查询时不把旧资源快照当作新结果，同一查询的后台重读可保留已完成展示。
- lot 目录（看板、首屏预取、标记页与历史场次）始终请求 `fields=summary` 精简投影，不下载装备属性明细；属性只在打开详情抽屉时经 `/api/v1/lots/{lot_id}` 读取。
- 整场标记集合按可读用户身份订阅会话变化：身份读取短暂失败后恢复时立即补读，同账号期限更新不重启集合请求；展示缓存不提供命令 CAS 版本，未知命令仍由页面宿主保留。
- 首次恢复被 Router 规范化的查询参数时，同时保留 URL 片段与 `history.state`；历史价格的平铺展示直接接管当前页行缓冲，分组展示只索引本页，不读取其他分页。
- 页头的人工刷新按钮常驻（不随连接状态或登录状态隐藏），点击后先展示忙碌状态再重新加载页面；自动轮询、恢复可见与实时失效只重读资源。外壳轮询、恢复可见、开场迁移、看板对齐重读、看板与履约进度页的错误重试，以及浏览器拒绝重新加载后的补读共用同一刷新入口（`crates/auction-web-public/src/surface/board/refresh.rs` 的 `refresh_public_reads`）：先使公开读缓存过期，再推进外壳刷新代号，TTL 内的条目也会带 ETag 回到服务端。浏览器拒绝重新加载时展示失败并补读权威目录。
- 公开读缓存（`crates/auction-web-public/src/api/swr_cache.rs`）在实时事件、命令答复与轮询时只把条目标为过期、保留正文与 ETag，随后的重读带 `If-None-Match`，正文未变时 304 直接复用；请求期间又发生失效时，新响应照样替换旧条目，只是按过期处理。重读遇到传输失败或暂时性拒绝（429、503、网关 502/504 与 CDN 520–524）时继续展示已有快照：lot 目录在 1 秒后再读一次，仍失败才保留旧快照，其他读取等下一次失效或轮询再读；履约进度页展示旧快照时另在表格上方标明该快照最近一次被服务端确认的时刻；没有快照时照常显示错误。竞价与代理命令得到答复（成功或被拒绝）后，命令之前的快照不再用于回退（答复发生在重读在途期间也一样：重读失败时按最新的回退下限重新判定），读视图只显示权威结果或错误；目录快照与当前场次不对齐时，同一上下文（场次、阶段、开放状态与预览修订号）只自动对齐重读一次：经共用刷新入口同时重读当前（或历史）场次与目录，两侧都返回后仍不对齐才显示可重试的错误，不再留空；场次或目录仍有读取在途、或上一轮为另一上下文发起的重读尚未返回时先等待，不发起新的重读（调度与组件调度测试见 `crates/auction-web-public/src/surface/board/alignment.rs`）。旧快照只用于展示，出价资格与金额仍由服务端在命令中判定。

## 本地偏好

`hv.lang`（语言）、`hv.theme`（主题）由共享层管理；目录显示列与每页数量等展示偏好保存在 `localStorage`，URL 中的显式查询参数优先。所有本地读写经共享 `storage` 边界（不可用或损坏时静默降级），DTO 解码的键集合与安全整数校验来自共享 `wire` 谓词。

## 结构

| 路径 | 内容 |
| --- | --- |
| `crates/auction-web-public/src/api/` | 同源 HTTP 访问与响应解码；每个文件对应一个 API 领域（`account`、`blacklist`、`clock`、`commands`、`compensation`、`forum`、`fulfillment`、`model`、`queries/`、`realtime/`、`registration`、`rules`、`session/`、`session_preference`、`status`、`watch_preference`、`watches`），`decoder/` 保存 wire DTO 解码 |
| `crates/auction-web-public/src/surface/` | 路由与页面外壳、看板、目录、分页（`pagination/` 的 `PublicPager` 注入 Public 文案、10/20/50/100 预设与契约页长上限 200，包装共享 `Pager`）、设备专属展示（`presentation/` 的桌面/手机），以及按领域分组的页面实现：`account/`、`commands/`、`reads/`、`compensation/`、`watches/`、`session_preference/`、`rules`、`forum`、`blacklist`、`realtime`、`pages/`、`board/`、`catalog/` |
| `crates/auction-web-public/assets/` | `crates/auction-web-public/assets/public.css`（组件与布局）、`crates/auction-web-public/assets/desktop.css`、`crates/auction-web-public/assets/mobile.css` |
| `crates/auction-web-public/i18n/` | Fluent 译文：`en`（默认，编译期内嵌）、`zh-CN`、`zh-TW`、`ja`、`ko`（作为带指纹的 `/assets/<lang>-dxh<hash>.ftl` 按需取回，失败保持当前语言） |

五种语言的消息键必须完全一致，代码引用的键必须存在于 `crates/auction-web-public/i18n/en.ftl`，键名不得含点号（`ci/checks/check.sh i18n` 检查）。

## 构建

```bash
cargo check --locked -p auction-web-public --target wasm32-unknown-unknown   # 编译检查
bash ci/release/web-bundle.sh public                               # 生成发布 payload（需要受控工具 dx）
```

`ci/release/web-bundle.sh` 先验证受控发布工具，再使用 `wasm-release` profile（体积优先、`panic = "abort"`）运行 `dx bundle`，输出到 `target/dx/build/auction-web-public/web/public` 并写入 payload 清单与哈希；源码、工具与构建环境指纹一致且已有 payload 通过门禁时才复用。随后构建 `auction-public-api` 的 `web-embed` 制品才能内嵌这份页面；单独编译 API 不生成 Web payload。前端只依赖契约 DTO 与共享浏览器机制，不启用 Dioxus fullstack/server。
