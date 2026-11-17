// 本地状态单元：响应式读取订阅写入，peek 同步返回最近一次写入的值。
// Solid 的信号在写入后要到下一次刷新才更新读取结果；命令状态机等命令式逻辑需要“写后立即可读”，
// 因此状态值保存在闭包变量里，信号只负责通知订阅者。

import { createSignal } from "solid-js";

/** 一份可读写的本地状态。 */
export interface Cell<T> {
  /** 响应式读取：在 JSX、memo 与 effect 的 compute 中订阅后续写入。 */
  (): T;
  /** 不订阅地读取最近一次写入的值；写入后同步可见。 */
  peek(): T;
  /** 写入新值并通知订阅者；新旧值相等时同样通知。 */
  set(value: T): void;
  /** 以当前值计算新值后写入；不得原地修改 current。 */
  update(change: (current: T) => T): void;
}

/** 创建一份本地状态；在组件体、状态对象的构造函数或模块作用域调用。 */
export function createCell<T>(initial: T): Cell<T> {
  let current = initial;
  // ownedWrite：effect 与组件初始化阶段的写入属于本状态的正常用法，不触发开发期的写入守卫。
  const [revision, setRevision] = createSignal(0, { ownedWrite: true });
  const cell = (() => {
    revision();
    return current;
  }) as Cell<T>;
  cell.peek = () => current;
  cell.set = (value) => {
    current = value;
    setRevision((count) => count + 1);
  };
  cell.update = (change) => {
    cell.set(change(current));
  };
  return cell;
}
