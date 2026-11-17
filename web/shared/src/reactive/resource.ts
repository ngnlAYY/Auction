// 随依赖重跑的异步读取：依赖变化时取消在途任务并重新执行，结果写入可订阅的状态。
// 不使用 Solid 的异步派生与 Loading 边界：页面需要显式区分“加载中、已就绪、已停止”，
// 并在重跑期间继续显示上一次结果。

import { createEffect } from "solid-js";
import type { Cell } from "./cell";
import { createCell } from "./cell";
import type { TaskContext, TaskHandle } from "./tasks";
import { createTaskScope } from "./tasks";

/** 读取任务的阶段。 */
export type ResourceState = "pending" | "ready" | "stopped";

/** 一份异步读取的结果与控制入口。 */
export interface TaskResource<T> {
  /** 响应式读取最近一次完成的结果；首次完成前与 clear() 之后为 undefined，重跑期间保留旧值。 */
  (): T | undefined;
  /** 不订阅地读取最近一次完成的结果。 */
  peek(): T | undefined;
  /** 响应式读取当前阶段。 */
  state(): ResourceState;
  /** 取消在途任务并以当前依赖重新执行。 */
  restart(): void;
  /** 取消在途任务，阶段置为 stopped；已有结果保留。 */
  cancel(): void;
  /** 丢弃已有结果；不影响在途任务。 */
  clear(): void;
}

/**
 * 创建随依赖重跑的异步读取。
 *
 * `dependencies` 同步读取任务需要的全部响应式值并原样返回；其中任何一个变化都会取消在途任务并
 * 重跑。`task` 不在订阅范围内：它只能使用收到的依赖值，await 之后读取其他状态要用 peek()。
 * 被取代或被取消的任务即使稍后完成，结果也会被丢弃。
 */
export function createTaskResource<D, T>(
  dependencies: () => D,
  task: (dependencies: D, context: TaskContext) => Promise<T>,
): TaskResource<T> {
  const value: Cell<T | undefined> = createCell<T | undefined>(undefined);
  const state = createCell<ResourceState>("pending");
  const scope = createTaskScope();
  let running: TaskHandle | undefined;
  let latestDependencies: D;

  const run = (current: D) => {
    running?.cancel();
    state.set("pending");
    const handle = scope.spawn(async (context) => {
      const result = await task(current, context);
      if (!context.alive()) {
        return;
      }
      value.set(result);
      state.set("ready");
    });
    running = handle;
  };

  createEffect(dependencies, (current) => {
    latestDependencies = current;
    run(current);
  });

  const resource = (() => value()) as TaskResource<T>;
  resource.peek = () => value.peek();
  resource.state = () => state();
  resource.restart = () => {
    run(latestDependencies);
  };
  resource.cancel = () => {
    running?.cancel();
    state.set("stopped");
  };
  resource.clear = () => {
    value.set(undefined);
  };
  return resource;
}
