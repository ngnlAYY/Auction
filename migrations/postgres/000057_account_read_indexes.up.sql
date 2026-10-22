-- Public 账户列表与总数均先按用户筛选；lot_id 同时供可见性联接，历史状态不能用 active 部分索引代替。
-- 竞价列表严格匹配 created_at DESC、id DESC；INCLUDE 避免计数联接仅为 lot_id 再读取竞价堆行。
CREATE INDEX bids_bidder_created_order_idx
    ON public.bids (bidder_user_id, created_at DESC, id DESC)
    INCLUDE (lot_id);

-- 代理历史列表严格匹配 updated_at DESC、lot_id ASC，保留终态授权与同一 lot 的全部使用历史。
CREATE INDEX bid_proxy_authorizations_user_updated_order_idx
    ON public.bid_proxy_authorizations (user_id, updated_at DESC, lot_id ASC);
