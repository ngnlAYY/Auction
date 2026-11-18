// 历史模块提交地址写入后，通知 Router 刷新实际地址的快照。

type LocationWrittenListener = () => void;

const listeners = new Set<LocationWrittenListener>();

/** 注册地址写入通知；返回注销函数。只供 Router 实例在创建时调用。 */
export function watchLocationWritten(listener: LocationWrittenListener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * 历史模块在地址写入成功后通知 Router。
 * Router 收到通知后刷新地址快照与当前历史条目标识，保留当前路由匹配和滚动处理。
 * 已销毁的 Router 不会再收到通知。
 */
export function notifyLocationWritten(): void {
  for (const listener of [...listeners]) {
    listener();
  }
}
