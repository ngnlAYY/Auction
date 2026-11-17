// 截断文本的共享尺寸监听：全页只注册一个 window `resize` 监听和一个 `ResizeObserver`，
// 各实例只登记自己的重测回调，卸载时注销；最后一个实例注销后释放全部监听。
// window 回调按登记顺序执行，与逐实例注册监听时的派发顺序一致。

type Measure = () => void;

interface SharedListeners {
  readonly window: Window;
  readonly observer: ResizeObserver;
  readonly resizeCallback: () => void;
  /** 元素到登记号的弱映射；不阻止已移除的 DOM 节点被回收。 */
  readonly ids: WeakMap<Element, number>;
}

interface Registry {
  nextId: number;
  readonly measures: Map<number, Measure>;
  listeners: SharedListeners | null;
}

const MAX_U32 = 0xffff_ffff;

const REGISTRY: Registry = {
  nextId: 0,
  measures: new Map(),
  listeners: null,
};

/** 登记守卫：销毁时停止观察元素并注销重测回调。 */
export interface MeasureSubscription {
  dispose(): void;
}

function install(): SharedListeners | null {
  if (typeof window === "undefined" || typeof ResizeObserver === "undefined") {
    return null;
  }
  let observer: ResizeObserver;
  try {
    observer = new ResizeObserver((entries) => dispatchEntries(entries));
  } catch {
    // 浏览器拒绝创建观察器时返回空守卫，实例保留挂载时的测量结果。
    return null;
  }
  const resizeCallback = dispatchAll;
  // 窗口监听注册失败时仍保留元素尺寸观察。
  try {
    window.addEventListener("resize", resizeCallback);
  } catch {
    // 仅保留观察器；元素尺寸变化仍会触发重测。
  }
  return {
    window,
    observer,
    resizeCallback,
    ids: new WeakMap(),
  };
}

function disposeListeners(listeners: SharedListeners): void {
  listeners.observer.disconnect();
  try {
    listeners.window.removeEventListener("resize", listeners.resizeCallback);
  } catch {
    // 窗口已不可用时没有可释放的监听。
  }
}

/** 观察元素尺寸并在窗口尺寸变化时重测；浏览器不支持时返回空，实例保留挂载时的测量结果。 */
export function observe(element: Element, measure: () => void): MeasureSubscription | null {
  if (REGISTRY.listeners === null) {
    REGISTRY.listeners = install();
    if (REGISTRY.listeners === null) {
      return null;
    }
  }
  if (REGISTRY.nextId >= MAX_U32) {
    return null;
  }
  const id = REGISTRY.nextId;
  REGISTRY.nextId = id + 1;
  const listeners = REGISTRY.listeners;
  listeners.ids.set(element, id);
  listeners.observer.observe(element);
  REGISTRY.measures.set(id, measure);
  let active = true;
  return {
    dispose() {
      if (!active) {
        return;
      }
      active = false;
      REGISTRY.measures.delete(id);
      const current = REGISTRY.listeners;
      if (current !== null) {
        // 同一元素重新挂载后已换成新登记号；旧守卫不能停止观察新登记。
        if (current.ids.get(element) === id) {
          current.ids.delete(element);
          current.observer.unobserve(element);
        }
      }
      if (REGISTRY.measures.size === 0 && REGISTRY.listeners !== null) {
        const released = REGISTRY.listeners;
        REGISTRY.listeners = null;
        disposeListeners(released);
      }
    },
  };
}

/** 回调前先复制登记表，重测过程中注销或新登记都不会改变本轮派发集合。 */
function dispatchAll(): void {
  const measures = Array.from(REGISTRY.measures.values());
  for (const measure of measures) {
    measure();
  }
}

function dispatchEntries(entries: readonly ResizeObserverEntry[]): void {
  const listeners = REGISTRY.listeners;
  if (listeners === null) {
    return;
  }
  const measures: Measure[] = [];
  for (const entry of entries) {
    const id = listeners.ids.get(entry.target);
    if (id === undefined) {
      continue;
    }
    const measure = REGISTRY.measures.get(id);
    if (measure !== undefined) {
      measures.push(measure);
    }
  }
  for (const measure of measures) {
    measure();
  }
}
