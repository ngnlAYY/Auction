// 用原生 Promise.withResolvers 管理外部异步完成，并兼容尚未实现该静态方法的浏览器。

interface PromiseResolvers<T> {
  readonly promise: Promise<T>;
  readonly resolve: (value: T | PromiseLike<T>) => void;
  readonly reject: (reason?: unknown) => void;
}

type NativePromiseConstructor = PromiseConstructor & {
  withResolvers?: <T>() => PromiseResolvers<T>;
};

/** 原生接口可用时直接创建响应器；旧浏览器使用等价的 Promise executor 回退。 */
export function createPromiseResolvers<T>(): PromiseResolvers<T> {
  // 项目类型目标仍为 ES2023；仅在运行时确认静态方法存在后调用。
  const nativePromise = Promise as NativePromiseConstructor;
  if (typeof nativePromise.withResolvers === "function") {
    return nativePromise.withResolvers<T>();
  }

  const captured: { resolvers: Pick<PromiseResolvers<T>, "resolve" | "reject"> | null } = {
    resolvers: null,
  };
  const promise = new Promise<T>((resolve, reject) => {
    captured.resolvers = { resolve, reject };
  });
  const resolvers = captured.resolvers;
  if (resolvers === null) {
    throw new Error("Promise 初始化器未执行");
  }
  return { promise, ...resolvers };
}
