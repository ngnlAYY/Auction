# auction-contracts

稳定的对外契约：REST/流 DTO、错误编码、分页和版本约定。

源码布局：`src/lib.rs` 只保留模块声明和公共导出；`src/contracts/mod.rs` 负责统一导出，
`src/contracts/dto/` 再按通用、Public 和 Admin 以及各自业务职责细分 DTO。Public catalog
继续由 `dto/public/catalog/{mod,auction,history,compensation,forum,lot}.rs` 分别持有拍卖上下文、
竞价/价格历史、补偿池、论坛镜像和 lot/装备响应，`catalog/mod.rs` 只保留稳定导出。

浏览器和外部客户端只能看到这里的类型。数据库记录和领域聚合都不得直接当作对外 DTO，
转换在边界完成。

- `HealthResponse`：健康检查响应。`service` 与 `version` 都由调用方传入——在本 crate 内展开
  `env!("CARGO_PKG_VERSION")` 只会得到契约 crate 自己的版本，而运维需要的是运行进程的版本。
  装配根应传入自身的 `CARGO_PKG_VERSION`。
- `ApiErrorResponse`：统一错误响应。
- `MAX_PUBLIC_BID_AMOUNT`：浏览器即时反馈使用的 1B 上限；HTTP 编译时核对它与领域常量一致，
  最终新命令仍由领域层校验。既有价格 DTO 字段保持不变，超过上限的 `next_bid` 表达不可继续出价。
- `AdminForumPreviewRequest` 与 `AdminForumPreviewResponse`：管理端论坛内容预览契约；输入
  只能选择一个非空的 HTML/文本字段，输出只包含安全的目录、帖子、命令和警告信息。
- `AdminSettlementReportResponse`：PostgreSQL 完整档的 Admin 结算安全投影；lot 明细显式返回
  `listed_price`、`hold_ppm` 与带符号的 `adjustment_ppm`，供界面区分折扣和惩罚，不暴露数据库行。
- `AdminSettlementCatalogResponse`：只包含已结算拍卖 UUID、world、期号、结算时间和有界分页，
  供 Admin 报表选择器使用，不复用包含生命周期命令字段的通用拍卖列表。
- `AdminWorldListResponse`、`AdminWorldSetEnabledRequest` 与 `AdminWorldSetEnabledResponse`：数据库
  权威业务世界的安全状态、`policy_revision` fence 和幂等启停命令；不表达 runtime 邮箱开关。
- `AdminRealtimeEventListResponse`：Admin 最近事件的分页元数据投影；只返回事件 ID、流、序号、
  类型、schema 与写入时间，不返回公开 payload、命令/竞价来源或数据库内部字段。
- Public lot/装备响应可选返回固定顺序的 `primary_attribute_initials`；竞价日志仅在真实论坛楼层与
  帖子身份同时可证明时返回受限的 `source_post_url`，玩家仍由名称字段展示。
- `PublicForumResponse.processed_through_post_number` 只表达 Worker 完整主题扫描证明的真实连续楼层；
  浏览器不得用 `content_revision`、post ID 或已读取末楼替代该水位。
- `PublicForumSyncStatusResponse`：P21 独立论坛同步开关，只包含必填布尔 `enabled`；未观测到可靠
  状态时使用 503 错误响应，不能把缺失值反序列化为 false，也不扩张 P15 的既有字段闭集。

字段命名必须先遵守已冻结的旧系统契约；新增契约使用 `snake_case` JSON 字段，并通过 serde
映射兼容外部旧命名。修改路由、状态码、JSON 字段或事件名时，必须同步更新对应文档。

本 crate 只依赖 serde，不引入 Axum、SQLx 或任何适配器类型。
