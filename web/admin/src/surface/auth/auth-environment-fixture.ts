// 认证测试的内存文档、属性恢复与受控定时器。

import { flush } from "solid-js";
import { preserveProperty } from "../../test-scope-fixture";
import { installTestDocument, TestDocument } from "../test-dom-fixture";

/** 带会话 Cookie 与页面级事件监听的内存文档。 */
export class AuthTestDocument extends TestDocument {
  hidden = false;
  get visibilityState(): "hidden" | "visible" {
    return this.hidden ? "hidden" : "visible";
  }
  readonly documentElement = null;
  readonly body = null;
  private readonly events = new EventTarget();
  readonly addEventListener = this.events.addEventListener.bind(this.events);
  readonly removeEventListener = this.events.removeEventListener.bind(this.events);

  private cookieValue: string;
  readonly cookieReads: (string | null)[] = [];

  constructor(cookie: string) {
    super();
    this.cookieValue = cookie;
  }

  get cookie(): string {
    const value = this.cookieReads.shift();
    if (value === null) throw new Error("测试文档无法读取 Cookie");
    return value ?? this.cookieValue;
  }

  set cookie(value: string) {
    this.cookieValue = value;
  }

  /** 模拟浏览器 visibilitychange 事件，不发起真实导航。 */
  setHidden(hidden: boolean): void {
    this.hidden = hidden;
    this.events.dispatchEvent(new Event("visibilitychange"));
  }
}

/** 在挂载前安装内存文档；任一个属性安装或恢复失败也会清理另一项。 */
export function installAuthDocument(cookie = "") {
  const document = installTestDocument(new AuthTestDocument(cookie));
  return { document, restore: document.restore };
}

const realSetTimeout = globalThis.setTimeout;

/** 等一轮真实宏任务，并提交 Solid 排队的状态更新。 */
export async function settleAuth(): Promise<void> {
  await new Promise((resolve) => realSetTimeout(resolve, 0));
  flush();
}

/** 等待按需加载的页面实际挂载标题，兼容测试替换全局定时器或固定日历时钟。 */
export async function waitForPageTitle(document: AuthTestDocument, id: string) {
  const deadline = Bun.nanoseconds() + 2_000_000_000;
  let title = document.getElementById(id);
  while (title === null) {
    if (Bun.nanoseconds() >= deadline) throw new Error(`等待页面标题挂载超时：${id}`);
    await settleAuth();
    title = document.getElementById(id);
  }
  return title;
}

/** 在用例内安装定时器，按精确延时手动派发并恢复原属性。 */
export function installManualTimers() {
  const timers = new Map<number, { callback: () => void; delay: number }>();
  const restoreSet = preserveProperty(globalThis, "setTimeout");
  const restoreClear = preserveProperty(globalThis, "clearTimeout");
  let next = 1;
  const restore = () => {
    try {
      restoreClear();
    } finally {
      restoreSet();
    }
  };
  try {
    Object.defineProperty(globalThis, "setTimeout", {
      configurable: true,
      writable: true,
      value: (callback: TimerHandler, delay = 0, ...args: unknown[]): number => {
        if (typeof callback !== "function") {
          throw new Error("定时器替身只接收函数");
        }
        const id = next++;
        timers.set(id, { callback: () => callback(...args), delay });
        return id;
      },
    });
    Object.defineProperty(globalThis, "clearTimeout", {
      configurable: true,
      writable: true,
      value: (id: number | undefined) => {
        if (id !== undefined) timers.delete(id);
      },
    });
    return {
      smallestDelay(): number | null {
        const delays = Array.from(timers.values(), ({ delay }) => delay).filter(
          (delay) => delay > 0,
        );
        return delays.length === 0 ? null : Math.min(...delays);
      },
      fire(delay: number): void {
        const due = [...timers.entries()].filter(([, timer]) => timer.delay === delay);
        if (due.length === 0) {
          throw new Error(`没有延时 ${delay} 的定时器`);
        }
        for (const [id, timer] of due) {
          timers.delete(id);
          timer.callback();
        }
      },
      restore,
    };
  } catch (error) {
    restore();
    throw error;
  }
}

/** 记录当前标签页的固定消息；可独立交付对端广播。 */
export function installAuthBroadcast() {
  const restore = preserveProperty(globalThis, "BroadcastChannel");
  const sent: string[] = [];
  const receivers = new Set<MemoryChannel>();
  class MemoryChannel {
    onmessage: ((event: MessageEvent) => void) | null = null;
    constructor(name: string) {
      if (name !== "hv-auction-admin-session") throw new Error(`意外的测试通道 ${name}`);
      receivers.add(this);
    }
    postMessage(value: string): void {
      sent.push(value);
    }
    close(): void {
      receivers.delete(this);
    }
  }
  Object.defineProperty(globalThis, "BroadcastChannel", {
    configurable: true,
    value: MemoryChannel,
  });
  return {
    sent,
    deliver(value = "session-ended"): void {
      for (const channel of receivers)
        channel.onmessage?.(new MessageEvent("message", { data: value }));
    },
    restore,
  };
}
