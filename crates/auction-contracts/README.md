# auction-contracts

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [契约](docs/contracts/README.md)

稳定 REST、实时消息、错误和分页 DTO；只拥有对外字段与序列化形状，不承载数据库行、应用读模型或领域写入。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/contracts/dto/public](crates/auction-contracts/src/contracts/dto/public) | Public 请求、响应与实时投影 |
| [src/contracts/dto/admin](crates/auction-contracts/src/contracts/dto/admin) | Admin 请求与安全响应 |
| [src/contracts](crates/auction-contracts/src/contracts) | 公共信封与稳定导出 |
| [src/contracts/setup.rs](crates/auction-contracts/src/contracts/setup.rs) | 独立首次部署输入、阶段与错误码 |

## 职责与实现边界

`lib.rs` 与 `contracts/mod.rs` 只负责导出，`contracts/dto/` 按通用、Public、Admin 和业务职责拆分；HTTP 与两端浏览器
复用这些 DTO，数据库行与领域聚合在边界转换。路由、状态码、字段语义与版本规则见
[HTTP 契约](docs/contracts/routes.md)和[实时契约](docs/contracts/events.md)，字段清单不在本页复制。

通用字段解码集中在 [dto/shared.rs](crates/auction-contracts/src/contracts/dto/shared.rs)，仅供 crate 内 DTO 使用：
`deserialize_required_option` 保留“字段必填、值可为 null”；`deserialize_null_default` 保留旧 Go 标量 null 到零值的映射，
缺失仍由各字段原有 `default` 控制。错误类型、未知字段拒绝及序列化形状不变，业务校验与错误分类仍归应用/HTTP。

## DTO 布局与消费方

[dto/public/watch.rs](crates/auction-contracts/src/contracts/dto/public/watch.rs) 定义装备标记的完整集合、CAS 请求和
可重放响应：持久 version 为正 i64，缺项才表示 false/version 0，取消保留正版本 tombstone，旧 channel 恒为 mm 占位；
[dto/public/watch_preference.rs](crates/auction-contracts/src/contracts/dto/public/watch_preference.rs) 定义账号渠道
GET/PUT（响应 user_id/channel/version，请求 command_id/channel/expected_version），未知字段一律拒绝。
两者都不把数据库任务行或凭据发给浏览器。

[dto/public/account.rs](crates/auction-contracts/src/contracts/dto/public/account.rs) 拥有 P29 的 `PublicRegistrationInfoResponse`，
只向登录页传递配置中的 bot 显示名，不与认证结果或完整 runtime 文档混用；字段见[注册提示契约](docs/contracts/routes.md#p29-注册账号提示)。

[dto/public/catalog/auction.rs](crates/auction-contracts/src/contracts/dto/public/catalog/auction.rs) 保留 v1 的
`AuctionSummaryResponse`；P30 显式使用平铺的 `AuctionSummaryV2Response`。两个版本复用泛型分页信封，默认类型仍为 v1，
论坛地址仅进入 v2，字段与消费顺序见[v2 拍卖历史契约](docs/contracts/routes.md#p30-v2-拍卖历史与论坛地址)。

[dto/admin/mail/progress.rs](crates/auction-contracts/src/contracts/dto/admin/mail/progress.rs) 拥有 A16 显式选择的安全邮件诊断；
与原始游标、进程内 claim 及数据库模型分离，旧请求不增加字段，字段与闭集见[邮件契约](docs/contracts/routes.md#邮件审查与轮询)。

[dto/admin/credentials.rs](crates/auction-contracts/src/contracts/dto/admin/credentials.rs) 拥有 A32 的互斥 Cookie/账号登录输入。
新增嵌套登录 DTO 的 Debug 同时隐藏用户名和密码，导入响应继续只有安全投影，不回传取得的 CK；
旧 `secret` 请求保持可用，精确字段和错误见[外部凭据契约](docs/contracts/routes.md#外部凭据)。

## 稳定投影

Admin 结算报告沿用原字段，`adjustment.policy` 可标识 `hold_curve_v2`。`compensation_amount/compensation_out`
始终表示奖池总支出；v2 仅为按全场名义需求同比缩放后的基础优惠，不追加额外优惠。v2 九变量账单显式标记模式，
以成交价加税费为抵扣前价，`pool_credit` 等于实分基础优惠；旧八变量仍只表示额外优惠。
金额及版本矩阵由领域/HTTP 校验，DTO 不重算策略；历史 v0/v1 形状不变，见[结算契约](docs/contracts/schema.md#结算恢复与交付)。

公开拍品的正式装备、预览摘要与预览详情装备使用必填 `equipment_group` 表达 `rare` / `fusion`；
DTO 只传递服务端结果，匹配逻辑与论坛颜色留在应用/集成层，见[装备分组契约](docs/contracts/routes.md#p09p11-装备分组)。

[dto/admin/equipment.rs](crates/auction-contracts/src/contracts/dto/admin/equipment.rs) 定义统一装备词典的窄只读 DTO；
`AdminRuntimeConfigResponse.equipment_taxonomy` 仅在三条 runtime 配置路由显式选择
`include_equipment_taxonomy=true` 时返回，省略时保持原字段集合。词典与 `document` 分离，
不进入 runtime TOML、fingerprint 或命令摘要，具体字段见[运行配置契约](docs/contracts/routes.md#runtime-配置与文件事务)。
确证目录 `kinds` 与配置相关的 `resolved_kinds` 分开：后者仅包含本次文档中经领域有限语法识别的完整种类及
对应词缀，最多 16 项、名称规范化去重，空集合省略且旧投影缺省为空；不会把可能组合加入确证目录。

论坛同步开关和镜像 DTO 由 `dto/public/catalog/forum.rs` 唯一拥有，Admin 认证状态 GET 复用同一安全字段形状而不复制 DTO。
成交价统计由 `dto/public/catalog/history.rs` 拥有；新增可选字符串 `median_exact` 表示规范十进制整数或 `.5`，
缺失时兼容原整数 `median`。精确金额使用字符串保留，不经 JSON 浮点数传输，详见[历史读取契约](docs/contracts/routes.md#标的编号来源与历史读取)。

- `HealthResponse` 的服务名和版本由装配根传入，不能用本 crate 版本代替运行进程版本。
- `ApiErrorResponse` 是统一安全错误信封，列表与 dashboard 只暴露声明的安全字段，不透传内部 payload。
- `SetupRequest`/`SetupResponse` 属于独立设置服务：`completed` 只表示配置保存，`initialize_and_start` 说明后续动作；
  `AdminRuntimeConfigResponse` 区分候选、fingerprint 与重启要求，`managed` 不表示热加载。
- `MAX_PUBLIC_BID_AMOUNT` 供浏览器即时反馈，最终金额校验仍由领域层完成；`PublicForumResponse` 的楼层水位与
  P21 布尔开关相互独立，未知开关通过错误表达而非默认 false。
- `PUBLIC_EXPECTED_USER_ID_HEADER` 统一 Public 写操作的可选身份前置条件名称；HTTP 核对当前 Cookie 身份，
  浏览器冻结确认宿主的用户，具体适用动作与错误码见[请求边界](docs/contracts/routes.md#请求边界)。
- `PublicLoginRequest`、`AdminSessionResponse` 和 `AdminUserMutationResponse` 保留原 JSON 与 `Debug` 能力，
  但调试输出隐藏登录 key、CSRF token 和生成 key，不改变授权请求/响应的实际字段。
- `PublicLoginResponse`、`PublicSessionCurrentResponse` 的截止字段为必填可空值，成对 `null` 表示永久，不用虚构远期时间；
  `PublicSessionPreferenceResponse` 与 `PublicSetSessionPreferenceRequest` 由 `dto/public/account.rs` 拥有，
  保存时必须显式提供 `duration_seconds`（可为 `null`），详见[登录时长契约](docs/contracts/routes.md#p27p28-登录时长)。
- `AdminIntakeRequest` 的来源 URL、正式/预览装备和补偿账本响应的完整游戏 URL，以及 `PublicForumResponse`
  的镜像正文在 `Debug` 中脱敏；这些字段可能包含装备 key。授权 JSON 输入/输出仍逐字保留原字段，不用脱敏值改写 wire。

## 命名与依赖边界

新增 JSON 字段用 `snake_case`，已发布名称通过 serde 映射兼容，字段/路由/错误/事件变化必须同步契约；依赖仅
serde、serde_json、chrono 与 uuid（见 [Cargo.toml](crates/auction-contracts/Cargo.toml)），不得引入 Axum、SQLx 或适配器类型。

## 维护与验证

```bash
cargo check --locked -p auction-contracts
```

仅静态编译；默认检查与禁止默认运行的测试范围见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。
