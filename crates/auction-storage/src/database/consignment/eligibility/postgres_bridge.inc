// 测试目标直接编译这些生产模块，避免为内部接口增加公开的测试导出。
pub(super) mod accounts;
pub(super) mod history;
pub(super) mod history_query;
pub(super) mod lots;
pub(super) mod occupancy;
pub(super) mod predicates;
