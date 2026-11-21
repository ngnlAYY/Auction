// Promise.withResolvers 缺失时仍能完成与拒绝等待中的操作。

import { expect, test } from "bun:test";

import { createPromiseResolvers } from "./promise";

function withoutNativePromiseWithResolvers<T>(run: () => T): T {
  const descriptor = Object.getOwnPropertyDescriptor(Promise, "withResolvers");
  Object.defineProperty(Promise, "withResolvers", { configurable: true, value: undefined });
  try {
    return run();
  } finally {
    if (descriptor === undefined) {
      Reflect.deleteProperty(Promise, "withResolvers");
    } else {
      Object.defineProperty(Promise, "withResolvers", descriptor);
    }
  }
}

test("原生 withResolvers 缺失时可完成等待", async () => {
  const promise = withoutNativePromiseWithResolvers(() => {
    const deferred = createPromiseResolvers<number>();
    deferred.resolve(42);
    return deferred.promise;
  });
  await expect(promise).resolves.toBe(42);
});

test("原生 withResolvers 缺失时保留拒绝结果", async () => {
  const failure = new Error("operation failed");
  const promise = withoutNativePromiseWithResolvers(() => {
    const deferred = createPromiseResolvers<number>();
    deferred.reject(failure);
    return deferred.promise;
  });
  await expect(promise).rejects.toBe(failure);
});
