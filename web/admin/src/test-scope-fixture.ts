// 临时替身与响应式挂载的清理作用域。

/** 保存属性描述符，使测试能恢复原值、属性特征或属性缺失的状态。 */
export function preserveProperty(target: object, key: PropertyKey): () => void {
  const previous = Object.getOwnPropertyDescriptor(target, key);
  return () => {
    if (previous === undefined) {
      Reflect.deleteProperty(target, key);
    } else {
      Object.defineProperty(target, key, previous);
    }
  };
}

/** 后安装的替身先恢复；任一恢复动作抛错时仍会执行先前的恢复动作。 */
export function cleanupScope(): { defer(cleanup: () => void): void; dispose(): void } {
  let disposeAll = (): void => {};
  return {
    defer(cleanup) {
      const previous = disposeAll;
      disposeAll = () => {
        try {
          cleanup();
        } finally {
          previous();
        }
      };
    },
    dispose() {
      const run = disposeAll;
      disposeAll = () => {};
      run();
    },
  };
}
