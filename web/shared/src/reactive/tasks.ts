// 与宿主同生命周期的异步任务。
// Promise 不能像被丢弃的 future 那样在 await 处停止，因此任务在每次 await 之后、写状态之前
// 必须检查 alive()；宿主销毁或任务被取消后 alive() 恒为 false，signal 同时中止在途请求。

import { onCleanup } from "solid-js";

/** 传给任务体的存活判定与取消信号。 */
export interface TaskContext {
  /** 任务仍可继续：宿主未销毁且任务未被取消。 */
  alive(): boolean;
  /** 任务被取消或宿主销毁时中止；传给 fetch 等可取消操作。 */
  readonly signal: AbortSignal;
}

/** 单个任务的句柄。 */
export interface TaskHandle {
  /** 取消任务：中止 signal，之后 alive() 为 false。重复调用无副作用。 */
  cancel(): void;
  /** 任务体已返回、抛出或被取消。 */
  finished(): boolean;
}

/** 宿主的任务集合。 */
export interface TaskScope {
  /** 启动一个任务；宿主已销毁时不执行任务体，返回已结束的句柄。 */
  spawn(task: (context: TaskContext) => Promise<void>): TaskHandle;
  /** 宿主是否仍然存活。 */
  alive(): boolean;
}

/**
 * 在当前组件或响应式作用域内创建任务集合；作用域销毁时取消其全部任务。
 * 必须在组件体或其他有宿主的初始化代码中调用，返回的 scope 可以在事件处理器里使用。
 */
export function createTaskScope(): TaskScope {
  let disposed = false;
  const running = new Map<AbortController, () => void>();
  onCleanup(() => {
    disposed = true;
    const entries = [...running];
    running.clear();
    for (const [controller, finish] of entries) {
      controller.abort();
      finish();
    }
  });
  return {
    alive: () => !disposed,
    spawn(task) {
      const controller = new AbortController();
      let finished = disposed;
      const finish = () => {
        finished = true;
        running.delete(controller);
      };
      const handle: TaskHandle = {
        cancel() {
          controller.abort();
          finish();
        },
        finished: () => finished,
      };
      if (disposed) {
        controller.abort();
        return handle;
      }
      running.set(controller, finish);
      const context: TaskContext = {
        alive: () => !controller.signal.aborted,
        signal: controller.signal,
      };
      // 任务体的异常等同程序错误：不在这里吞掉，交给全局未处理拒绝钩子显示错误横幅。
      let promise: Promise<void>;
      try {
        promise = task(context);
      } catch (error) {
        finish();
        throw error;
      }
      void promise.finally(finish);
      return handle;
    },
  };
}
