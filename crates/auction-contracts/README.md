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
- `AdminForumPreviewRequest` 与 `AdminForumPreviewResponse`：管理端论坛内容预览契约；输入
  只能选择一个非空的 HTML/文本字段，输出只包含安全的目录、帖子、命令和警告信息。
- `AdminSettlementReportResponse`：PostgreSQL 完整档的 Admin 结算安全投影；lot 明细显式返回
  `listed_price`、`hold_ppm` 与带符号的 `adjustment_ppm`，供界面区分折扣和惩罚，不暴露数据库行。

字段命名必须先遵守已冻结的旧系统契约；新增契约使用 `snake_case` JSON 字段，并通过 serde
映射兼容外部旧命名。修改路由、状态码、JSON 字段或事件名时，必须同步更新对应文档。

本 crate 只依赖 serde，不引入 Axum、SQLx 或任何适配器类型。
