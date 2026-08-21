# auction-contracts

稳定的对外契约：REST/流 DTO、错误编码、分页和版本约定。

浏览器和外部客户端只能看到这里的类型。数据库记录和领域聚合都不得直接当作对外 DTO，
转换在边界完成。

- `HealthResponse`：健康检查响应。
- `ApiErrorResponse`：统一错误响应。

字段命名必须先遵守已冻结的旧系统契约；新增契约使用 `snake_case` JSON 字段，并通过 serde
映射兼容外部旧命名。修改路由、状态码、JSON 字段或事件名时，必须同步更新对应文档。

本 crate 只依赖 serde，不引入 Axum、SQLx 或任何适配器类型。
