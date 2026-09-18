# auction-setup

本 crate 只拥有 PostgreSQL 单机部署的首次部署 HTTP 入口。它在 loopback 上提供一次性
`/setup` 页面及受随机令牌保护的校验/完成接口；不挂载普通管理命令，也不迁移数据库。
表单分别接收数据库主机、端口、账号、密码和库名，再映射到 `auction-config` 的连接模型。
连接串生成、配置模板、敏感文件写入和三角色校验由 `auction-config` 拥有，数据库连通性由
`auction-storage` 检查。完成后进程退出，再由部署控制器执行严格预检、迁移和正式启动。
