# auction-setup

[文档导航](../../docs/README.md) · [初始化契约](../../docs/contracts/routes.md#首次部署初始化) · [实现与验证状态](../../docs/status.md)

本 crate 只拥有 PostgreSQL 单机部署的首次部署 HTTP 入口。它默认在 loopback 上提供一次性
`/setup` 页面及受随机令牌保护的部署码验证、配置校验/完成接口；不挂载普通管理命令，也不迁移数据库。
`POST /api/v1/setup/verify` 无请求体，仅检查 `x-setup-token` 与操作状态，不访问数据库或文件，
不消费令牌或建立全局解锁状态。页面收到 `verified` 后才开放配置步骤；后续 `validate` 和
`complete` 仍逐请求校验令牌，正式 Admin 不注册这三个接口。
三个接口先鉴权，再于读取正文前获取同一非阻塞操作锁，竞争返回 409 `setup_busy`；配置检查和保存从输入校验到
数据库等待、目录发布和终态写入都持有该锁，避免并发检查在保存后误报可继续配置。保存成功后
到进程退出前的新请求返回 409 `setup_completed`；目录发布后持久化确认失败则进入本进程不可逆的
结果不确定状态，后续已鉴权请求均返回 500 `setup_outcome_unknown`，不再连接数据库或写配置。
此时进程留待操作者停止并核对目标目录，不能靠再次验证部署码解除状态；停止时返回失败而非成功。
启动时只有输出路径明确不存在才绑定监听器，无法读取元数据也失败关闭。
表单分别接收数据库主机、端口、账号、密码和库名，再映射到 `auction-config` 的连接模型。
正式服务另接受可省略的 `public_listener_port`/`admin_listener_port`，默认 8090/8091；传给
配置 owner 校验非零且不同后，仅生成回环监听地址，Admin 角色与 runtime 同步。它们不改变
本次设置服务的 listener 或 Origin/代理门禁；API 不探测端口占用和绑定权限。
可省略的 `schedule` 对象接受时区与预览/开拍/结束的星期及时间，缺省沿用旧计划；顶层
`schedule_enabled` 控制启用。HTTP 只映射到配置 owner 的 `AuctionScheduleConfig`，世界固定为
`main`、`isekai`，即使关闭也在连库前校验候选；不修改已有拍卖或在设置接口中执行调度。
连接串生成、配置模板、敏感文件写入和三角色校验由 `auction-config` 拥有，数据库连通性由
`auction-storage` 检查。配置 owner 在私有临时目录内固定非敏感文件 `0640`、敏感文件 `0600`，
并在发布前复用正式 runtime 文件读取校验；`umask 077` 不会把已发布的 runtime 文件变为不可用。
角色 loader 会先固定主配置的绝对位置，再解析相对引用，适用于后续默认发现生成目录的启动方式。
默认分体完成后进程退出；随后用 `auctionctl deployment-check` 严格预检，
由 `auctionctl deployment start` 在三角色停机门禁通过后迁移并正式启动；也可先用受保护的
`auctionctl migrate` 独立迁移。设置 API 和单角色普通启动都不执行迁移。

`run_first_run_with_policy` 为可选合并入口返回 `Completed`、`Interrupted`、`OutcomeUnknown`；
只有 Completed 允许装配根进入首次空库初始化。API 本身不迁移，成功响应的
`initialize_and_start` 仅说明后续动作，不代表服务已就绪；合并 complete 要求
`confirm_initialize=true`，否则 400 `setup_confirmation_required`。
`server/access.rs` 拥有默认 loopback Host/Origin 门禁，以及显式 HTTPS origin/受信代理 IP 的
远程策略；不信任任意转发头，不匹配返回 403 `setup_origin`。所有应用响应，包括门禁拒绝、
框架错误及超时，均由同一出口补齐 `no-store` 与 `auction-http` 的浏览器安全头，包含
`frame-ancestors 'none'`、`X-Frame-Options: DENY` 和 `no-referrer`，不复制或放宽 Dioxus CSP。
`server/lifecycle.rs` 复用 `auction-http::serve_http` 的固定 5 秒 HTTP/1 请求头限制与连接任务所有权；
`access.rs` 给正文读取与异步处理共计 10 秒预算，超时为 504 `request_timeout`，保存仍按未知结果处理。
停止接收后由 `auction-process` 最多排空 10 秒，二次信号或超时会取消连接；终止信号不会触发
后续数据库初始化，强制收口后非阻塞核对终态，无法确认则失败关闭。同步目录发布到终态写入之间
没有 await，不能把请求取消解释为目录未发布。这里不承诺操作系统阻塞磁盘调用可以被异步 deadline 抢占。
已有或损坏配置不降级为设置页；正常 Admin 仍不注册上述写接口。
