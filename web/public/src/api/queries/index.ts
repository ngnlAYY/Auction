// Public 拍卖、lot、竞价日志和价格历史查询。
// cached-get 拥有条件 GET、非 2xx 响应的缓存分类与带缓存的严格解码，其余子模块按读取对象
// 划分：current（世界列表、当前场次与按 ID 打开的历史场次）、directory（场次目录分页）、
// bid-log（竞价日志与按状态的场次目录遍历）、lots（lot 目录与详情）、price-history（价格历史）。
//
// 源 queries/mod.rs 只含子模块声明与重导出；按迁移对照手册，纯重导出不迁移。
