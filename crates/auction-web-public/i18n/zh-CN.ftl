# 公开端简体中文文案。

app-title = 拍卖
language-label = 语言

nav-auctions = 拍卖
nav-my-bids = 我的竞价
nav-status = 服务状态
nav-label = 主导航

auction-title = 拍卖
auction-world-main = 主世界
auction-status-live = 进行中
auction-ends-at = 结束于
auction-round-eyebrow = 当前拍卖轮次
auction-round-title = 第 24 轮装备拍卖
auction-round-note = 竞价结果会实时更新，出价前请确认拍品编号和当前价格。

stats-lots = 拍品
stats-bids = 当前出价

filter-label = 拍品分类
filter-all = 全部
filter-one-handed = 单手
filter-two-handed = 双手
filter-other = 其他

category-one-handed = 单手
category-code-one = one

catalog-code = 编号
catalog-item = 装备
catalog-status = 状态
catalog-price = 当前价
catalog-remaining = 剩余时间
catalog-actions = 操作
catalog-view = 查看
catalog-bid = 出价

status-live = 竞价中
status-sold = 已售出
status-ended = 已结束

migration-note = 页面样式已按 Auction-BF 公共拍卖页迁移；数据接口接入后将替换示例数据。
data-loading = 正在加载拍卖数据……
data-error = 拍卖数据暂时不可用。
data-empty = 当前没有进行中的拍卖。

not-found-title = 页面不存在
not-found-note = 请检查链接是否正确。

page-auctions-title = 拍卖列表
page-auctions-note = 轮次列表将在拍卖查询用例接入后展示。
page-lot-title = 拍品 { $code }
page-lot-note = 拍品详情与出价将在竞价命令契约冻结后接入。
page-bidlog-title = 竞价记录
page-bidlog-note = 竞价记录来自公开竞价日志投影。
page-history-title = 历史轮次
page-history-note = 已成交记录来自公开价格历史投影。
page-login-title = 登录
page-login-note = 登录将在会话能力迁移后接入。
page-me-title = 我的账户
page-me-note = 账户信息将在用户能力迁移后接入。
page-account-title = 我的竞价
page-account-note = 登录和竞价历史将在账户能力迁移后接入。
page-rules-title = 拍卖规则
page-rules-note = 下方展示当前发布的规则文档。
page-blacklist-title = 黑名单
page-blacklist-note = 下方展示当前公开黑名单。
page-compensation-pool-title = 补偿池
page-compensation-pool-note = 展示已发布的结算批次和当前补偿池余额。
page-compensation-title = 补偿记录
page-compensation-note = 展示最新已发布的补偿账本。
compensation-balance = 补偿池余额：{ $balance }
compensation-updated = 更新时间：{ $updated_at }
compensation-auction = 拍卖
compensation-opening = 期初余额
compensation-tax = 税收
compensation-out = 补偿支出
compensation-closing = 期末余额
compensation-movements = 流水数
compensation-settled = 结算时间
compensation-round = { $world } 第 { $sequence } 轮
compensation-sequence = 序号
compensation-kind = 类型
compensation-lot = 拍品
compensation-amount = 金额
compensation-balance-after = 发生后余额
compensation-created = 创建时间
forum-title = 论坛镜像
forum-status = 同步状态：{ $status } · 源版本 { $revision }
forum-updated = 更新时间：{ $updated_at }
forum-link = 打开论坛帖子
page-status-title = 服务状态
page-status-note = 下方展示服务健康状态和持久化实时事件。
health-service = 服务
health-status = 状态
health-version = 版本

bidlog-lot = 拍品
bidlog-bidder = 竞价者
bidlog-amount = 金额
bidlog-mechanism = 竞价方式
bidlog-valid = 已接受
bidlog-invalid = 无效
bidlog-time = 时间

history-world = 世界
history-round = 轮次
history-item = 装备
history-buyer = 买家
history-seller = 卖家
history-price = 价格
history-closed = 成交时间
history-statistics = 价格统计
history-count = 数量
history-min = 最低
history-max = 最高
history-median = 中位数

login-key-label = 登录凭据
login-submit = 登录
login-working = 正在登录……
login-success = 已以 { $username } 身份登录。
login-error = 登录失败，请检查凭据后重试。
session-loading = 正在检查当前会话……
session-error = 会话服务暂时不可用。
session-anonymous = 当前未登录。
session-user = 当前登录身份：{ $username }。
session-expires = 会话将于 { $expires_at } 过期。
logout = 注销
logout-success = 已注销。
logout-error = 注销失败，请重试。

account-login-required = 登录后才能查看竞价记录。
account-total = { $bids } 条竞价 · { $proxies } 个代理授权
account-no-bids = 暂无竞价记录。
account-no-proxies = 暂无代理授权。
account-bids-title = 竞价历史
account-proxies-title = 代理授权
account-lot = 拍品
account-item = 装备
account-amount = 金额
account-mechanism = 竞价方式
account-leading = 领先
account-leading-yes = 是
account-leading-no = 否
account-time = 时间
account-mode = 模式
account-maximum = 上限
account-quota = 配额
account-status = 状态
account-capability = 可执行能力
account-capability-lower = 可降低
account-capability-locked = 已锁定

bid-amount-label = 出价金额
bid-submit = 提交出价
bid-working = 正在提交……
bid-success = 出价已接受，当前价格为 { $price }（版本 { $version }）。
bid-error = 出价未被接受，请检查拍品状态和金额。

rules-revision = 语言：{ $locale }，版本：{ $revision }。
rules-published = 发布时间：{ $published_at }。
blacklist-total = 共 { $total } 条公开记录
blacklist-empty = 当前公开黑名单为空。
blacklist-player = 玩家标识
blacklist-reason = 公开原因
blacklist-time = 加入时间

realtime-loading = 正在加载持久化实时事件……
realtime-connection-checking = 正在检查公开会话，以准备实时更新……
realtime-connection-connecting = 正在连接实时更新……
realtime-connection-connected = 实时更新已连接。
realtime-connection-anonymous = 登录后才能接收实时更新；仍可使用事件追赶读取。
realtime-connection-stopped = 实时更新已停止，请重新读取权威快照。
realtime-connection-error = 实时更新暂不可用，请刷新以读取快照。
realtime-error = 持久化实时事件暂时不可用。
realtime-empty = 当前没有持久化实时事件。
realtime-refresh = 刷新事件
realtime-load-more = 继续读取事件
realtime-cursor = 当前游标已推进到序号 { $sequence }。
realtime-refetch-required = 当前事件流需要先重新读取权威快照，暂不能继续应用。
realtime-event-sequence = 序号
realtime-event-kind = 事件
realtime-event-schema = Schema
realtime-event-payload = 公开负载
realtime-event-time = 时间

proxy-title = 代理授权
proxy-loading = 正在加载代理授权……
proxy-load-error = 代理授权暂不可用，请登录后重试。
proxy-create-maximum-label = 代理上限
proxy-create-mode-label = 代理模式
proxy-mode-fast = 快速
proxy-mode-slow = 慢速
proxy-create-submit = 创建代理授权
proxy-working = 正在提交……
proxy-create-success = 代理授权已接受，上限为 { $maximum }（版本 { $version }）。
proxy-command-error = 代理命令未被接受，请检查拍品状态和金额。
proxy-current = 当前代理授权
proxy-status = 状态：{ $status }
proxy-maximum = 上限：{ $maximum }
proxy-lower-label = 新代理上限
proxy-lower-submit = 降低上限
proxy-lower-success = 代理上限已降至 { $maximum }（版本 { $version }）。
