# auction-process

三个可部署进程共用的操作系统生命周期原语。

该 crate 只负责订阅 SIGINT/SIGTERM、区分首次与二次终止信号、执行默认 10 秒有界 drain，
以及用 abort-on-drop 守卫防止 Tokio 任务在强制收口时脱离。它不读取配置、不打开监听器、
不发布心跳，也不决定任何业务任务的停止顺序；这些所有权仍属于 `apps/public-api`、
`apps/admin-api` 与 `apps/worker` 装配根。

非 Unix 平台使用 Tokio 的 Ctrl-C 抽象。Unix 平台显式订阅 SIGINT 与 SIGTERM，使本地终止和
systemd 默认停止信号进入同一条路径；drain 期间第二次信号会要求装配根立即强制收口。
