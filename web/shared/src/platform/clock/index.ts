// 秒级共享时钟。
// 外壳创建单一 interval 并经 context 下发，消费方订阅同一计数信号，避免同屏多个一秒循环叠加；
// 本模块不引入服务端时钟校准业务。

import { type Accessor, createContext, onCleanup, useContext } from "solid-js";

import { createCell } from "../../reactive/cell";

const TICKER_INTERNALS = Symbol("second-ticker-internals");

interface TickerState {
  subscribers: number;
  interval: number | null;
}

/** 供 `useSecondTick` 使用的隐藏订阅能力；消费方只读取 `tick`。 */
interface TickerInternals {
  subscribe(): () => void;
}

/** 外壳持有的秒级时钟句柄，由 `useSecondTicker` 创建并放入 `SecondTickerContext`。 */
export interface SecondTicker {
  /** 每秒递增一次的修订计数；不代表具体时间戳。 */
  readonly tick: Accessor<number>;
  readonly [TICKER_INTERNALS]: TickerInternals;
}

/** 外壳把 `useSecondTicker()` 的结果放入此上下文，供整棵子树订阅同一节拍。 */
export const SecondTickerContext = createContext<SecondTicker>();

/** 从 0 到 1 个订阅者时启动 interval，回到 0 时停止。 */
function startInterval(
  state: TickerState,
  tick: Accessor<number>,
  setTick: (value: number) => void,
): void {
  if (typeof window === "undefined") {
    return;
  }
  const handle = window.setInterval(() => {
    // 修订号只用于通知；安全整数上限之后保持最大值并继续通知。
    setTick(Math.min(tick() + 1, Number.MAX_SAFE_INTEGER));
  }, 1_000);
  state.interval = handle;
}

/** 在外壳根组件调用一次：创建秒级时钟；根组件须把返回值放入 `SecondTickerContext`。 */
export function useSecondTicker(): SecondTicker {
  const tickCell = createCell(0);
  const state: TickerState = { subscribers: 0, interval: null };
  const tick: Accessor<number> = () => tickCell();
  const internals: TickerInternals = {
    subscribe() {
      state.subscribers += 1;
      if (state.subscribers === 1 && state.interval === null) {
        startInterval(state, tick, (value) => tickCell.set(value));
      }
      let active = true;
      return () => {
        if (!active) {
          return;
        }
        active = false;
        state.subscribers = Math.max(0, state.subscribers - 1);
        if (state.subscribers === 0 && state.interval !== null) {
          window.clearInterval(state.interval);
          state.interval = null;
        }
      };
    },
  };
  return {
    tick,
    [TICKER_INTERNALS]: internals,
  };
}

/** 订阅外壳提供的秒级时钟；消费方在派生计算里读取返回值即可每秒重跑。 */
export function useSecondTick(): Accessor<number> {
  const ticker = useContext(SecondTickerContext);
  // 沿源码的订阅守卫语义：订阅在组件创建时生效，卸载时退订；不要推迟到首次渲染完成。
  onCleanup(ticker[TICKER_INTERNALS].subscribe());
  return ticker.tick;
}
