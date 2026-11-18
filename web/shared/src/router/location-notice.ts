// 绕过 Router 直接写 History 后的地址快照通知。
// 本模块只转发“地址栏可能已变”的信号；读取地址、解析路由与滚动恢复仍由 Router 自己负责。
//
// 谁该通知：只有“冷加载时把被路由规范化掉的查询参数恢复回地址栏”这类先于页面内容渲染的写入。
// 页面运行中由筛选、分页等发起的查询写回不通知——Dioxus 的 Link 只在自身渲染时读取地址栏，
// 那些写回不会让它重渲染，所以导航链接的“当前页”标记在写回之后保持不变。两种做法的可见差别：
// 冷加载 /bidlog?auction=… 时“出价记录”链接不带 aria-current；冷加载 /bidlog 后页面自行补上
// 查询参数时，该链接仍带 aria-current。两者都与 Rust 版逐页对照一致。

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
 * 通知已绕过 Router 直接写入 History 的模块：地址栏内容可能变化。
 * Router 收到通知后只重读 pathname + search + hash 并刷新 currentLocationString()；
 * 不重新匹配路由、不改变 current()，也不触发滚动处理。已销毁的 Router 不会再收到通知。
 */
export function notifyLocationWritten(): void {
  for (const listener of [...listeners]) {
    listener();
  }
}
