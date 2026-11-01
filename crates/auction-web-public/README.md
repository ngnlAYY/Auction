# auction-web-public

[文档导航](../../docs/README.md) · [设计约定](DESIGN.md) · [玩家指南](../../docs/guide/player-guide.md) · [HTTP 路由](../../docs/contracts/routes.md)

玩家使用的公开站点：Dioxus 0.7 客户端渲染（CSR）WASM 应用，构建产物由 `auction-public-api` 的 `web-embed` 构建内嵌并同源提供。页面标题固定为“Auction 公开站点”（发布门禁以此识别 payload）。

## 页面

| 路径 | 页面 |
| --- | --- |
| `/`、`/auction` | 拍卖看板：按世界切换，当前或历史场次的预览/正式目录、倒计时、实时连接状态，按装备分组（稀有/融合）、类型与等级筛选，可配置显示列，在行内或弹窗中出价、设置/下调代理、标记装备 |
| `/auctions` | 历史拍卖列表 |
| `/lots/{lot_code}` | lot 详情：参数实际为 lot UUID；展示装备属性、游戏装备链接（如可用）、竞价历史与操作 |
| `/bidlog` | 出价日志 |
| `/history` | 成交价格历史：名称/日期/金额筛选、分组与中位数 |
| `/login` | 用登录 key 登录（支持 `return_to`） |
| `/me`、`/account` | 我的：当前标记、出价与代理，登录时长设置，竞价提醒渠道 |
| `/rules` | 规则文档（Markdown，五种语言） |
| `/blacklist` | 黑名单 |
| `/compensation-pool`、`/compensation` | 各世界补偿池余额、各场收支与逐件结算明细（含成交撤销冲正） |
| `/forum` | 论坛同步摘要 |
| `/status` | 服务说明 |

其他路径显示 404 页面。新增页面须同步修改 `crates/auction-web-public/src/app/surface/router.rs` 的 `Route`、后端 [`crates/auction-http/src/assets/routes.rs`](../auction-http/src/assets/routes.rs) 与 [路由文档](../../docs/contracts/routes.md)。

## 数据与实时

- 业务请求走同源 `/api/v1/*`，外壳另读 `/health/ready`；HTTP 经 `auction-web-shared` 的传输边界（默认 10 秒响应头与正文读取预算、可取消），响应按路由专属 decoder 解析为 `auction-contracts` DTO。
- 登录用户的看板与 lot 详情通过 `/api/v1/ws` WebSocket 接收实时事件（未登录时页头显示手动刷新模式）。连接建立后先用 `/api/v1/events` 补齐，序列跳号时重新补齐；断线按 1 秒起步、翻倍至 30 秒封顶并带随机抖动的退避重连，连续快速失败后降级为手动刷新；页面隐藏时暂停轮询与重连退避。普通竞价事件对相关资源做 100ms 合并的有界重读，生命周期变化或回放缺口触发权威快照刷新，不在前端推导竞价结果。
- 竞价、代理与标记命令带稳定命令 ID；网络结果不确定时保留原负载供“原样重试”，不会生成新命令。标记、警告渠道与登录时长设置只有各自路由的明确 `409` 冲突码才解除未知状态；损坏响应、未识别的 `409` 与后续前置拒绝不能否定之前的提交。
- 会话 Cookie 由后端以 HttpOnly 下发；跨标签页通过 `storage` 事件触发权威身份重读，Cookie 写入共用原生 Web Lock。会话读取、Cookie 同步与身份变更的边界见 [设计约定](DESIGN.md#会话)。
- 列表响应绑定世界、场次、筛选、页码与每页数量等完整请求身份；切换查询时不把旧资源快照当作新结果，同一查询的后台重读可保留已完成展示。
- 整场标记集合按可读用户身份订阅会话变化：身份读取短暂失败后恢复时立即补读，同账号期限更新不重启集合请求；展示缓存不提供命令 CAS 版本，未知命令仍由页面宿主保留。
- 首次恢复被 Router 规范化的查询参数时，同时保留 URL 片段与 `history.state`；历史价格的平铺展示直接接管当前页行缓冲，分组展示只索引本页，不读取其他分页。
- 页头人工刷新先展示忙碌状态再重新加载页面；自动轮询、恢复可见与实时失效只重读资源。浏览器拒绝重新加载时展示失败并补读权威目录。

## 本地偏好

`hv.lang`（语言）、`hv.theme`（主题）由共享层管理；目录显示列与每页数量等展示偏好保存在 `localStorage`，URL 中的显式查询参数优先。

## 结构

| 路径 | 内容 |
| --- | --- |
| `crates/auction-web-public/src/api/` | 同源 HTTP 访问与响应解码 |
| `crates/auction-web-public/src/app/surface/` | 路由与页面外壳、看板、目录、分页、设备专属展示（桌面/手机） |
| `crates/auction-web-public/src/app/auction/` | 竞价与代理命令、只读历史、装备标记 |
| `crates/auction-web-public/src/app/identity/` | 登录、账户、登录时长与提醒偏好 |
| `crates/auction-web-public/src/app/content/` | 规则、黑名单、补偿池、论坛摘要 |
| `crates/auction-web-public/assets/` | `crates/auction-web-public/assets/public.css`（组件与布局）、`crates/auction-web-public/assets/desktop.css`、`crates/auction-web-public/assets/mobile.css` |
| `crates/auction-web-public/i18n/` | Fluent 译文：`en`（默认）、`zh-CN`、`zh-TW`、`ja`、`ko` |

五种语言的消息键必须完全一致，代码引用的键必须存在于 `crates/auction-web-public/i18n/en.ftl`，键名不得含点号（`ci/checks/check.sh i18n` 检查）。

## 构建

```bash
cargo check --locked -p auction-web-public --target wasm32-unknown-unknown   # 编译检查
bash ci/release/web-bundle.sh public                               # 生成发布 payload（需要受控工具 dx）
```

`ci/release/web-bundle.sh` 先验证受控发布工具，再使用 `wasm-release` profile（体积优先、`panic = "abort"`）运行 `dx bundle`，输出到 `target/dx/build/auction-web-public/web/public` 并写入 payload 清单与哈希；源码、工具与构建环境指纹一致且已有 payload 通过门禁时才复用。随后构建 `auction-public-api` 的 `web-embed` 制品才能内嵌这份页面；单独编译 API 不生成 Web payload。前端只依赖契约 DTO 与共享浏览器机制，不启用 Dioxus fullstack/server。
