# auction-setup

[文档导航](../../docs/README.md) · [初始化契约](../../docs/contracts/routes.md#首次部署初始化) · [实现与验证状态](../../docs/status.md)

本 crate 只拥有 PostgreSQL 单机部署的首次部署 HTTP 入口。它默认在 loopback 上提供一次性
`/setup` 页面及受随机令牌保护的部署码验证、配置校验/完成接口；不挂载普通管理命令，也不迁移数据库。
`POST /api/v1/setup/verify` 无请求体，仅检查 `x-setup-token` 与操作状态，不访问数据库或文件，
不消费令牌或建立全局解锁状态。页面收到 `verified` 后才开放配置步骤；后续 `validate` 和
`complete` 仍逐请求校验令牌，正式 Admin 不注册这三个接口。
三个接口在鉴权后获取同一非阻塞操作锁，竞争返回 409 `setup_busy`；配置检查和保存从输入校验到
数据库等待、目录发布和终态写入都持有该锁，避免并发检查在保存后误报可继续配置。保存成功后
到进程退出前的新请求返回 409 `setup_completed`；目录发布后持久化确认失败则进入本进程不可逆的
结果不确定状态，后续已鉴权请求均返回 500 `setup_outcome_unknown`，不再连接数据库或写配置。
此时进程留待操作者停止并核对目标目录，不能靠再次验证部署码解除状态；停止时返回失败而非成功。
启动时只有输出路径明确不存在才绑定监听器，无法读取元数据也失败关闭。
表单分别接收数据库主机、端口、账号、密码和库名，再映射到 `auction-config` 的连接模型。
连接串生成、配置模板、敏感文件写入和三角色校验由 `auction-config` 拥有，数据库连通性由
`auction-storage` 检查。默认分体完成后进程退出；随后用 `auctionctl deployment-check` 严格预检，
由 `auctionctl deployment start` 在三角色停机门禁通过后迁移并正式启动；也可先用受保护的
`auctionctl migrate` 独立迁移。设置 API 和单角色普通启动都不执行迁移。

`run_first_run_with_policy` 为可选合并入口返回 `Completed`、`Interrupted`、`OutcomeUnknown`；
只有 Completed 允许装配根进入首次空库初始化。API 本身不迁移，成功响应的
`initialize_and_start` 仅说明后续动作，不代表服务已就绪；合并 complete 要求
`confirm_initialize=true`，否则 400 `setup_confirmation_required`。
`server/access.rs` 拥有默认 loopback Host/Origin 门禁，以及显式 HTTPS origin/受信代理 IP 的
远程策略；不信任任意转发头，不匹配返回 403 `setup_origin`。契约要求全部响应禁止缓存；
当前拒绝路径提前返回而遗漏安全头，已登记为[实现缺口](../../docs/status.md#源码核对发现的实现缺口)，不能将其视为已满足契约。
已有或损坏配置不降级为设置页；正常 Admin 仍不注册上述写接口。
