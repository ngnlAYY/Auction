// 响应式内核的行为断言：写后立即可读、等值写入仍通知、按值去重、任务随宿主取消、异步读取随依赖重跑。

import { expect, test } from "bun:test";
import { createEffect, createRoot, flush } from "solid-js";
import { createCell } from "./cell";
import { structuralEquals } from "./equality";
import { createValueMemo } from "./memo";
import { createTaskResource } from "./resource";
import { createTaskScope } from "./tasks";

const sleep = (milliseconds: number) =>
  new Promise<void>((resolve) => setTimeout(resolve, milliseconds));

test("cell：写入后同步可读，等值写入仍通知订阅者", () => {
  createRoot((dispose) => {
    const count = createCell(0);
    count.set(1);
    expect(count.peek()).toBe(1);
    expect(count()).toBe(1);

    let runs = 0;
    createEffect(
      () => count(),
      () => {
        runs += 1;
      },
    );
    flush();
    expect(runs).toBe(1);
    count.set(1);
    flush();
    expect(runs).toBe(2);
    count.update((current) => current + 1);
    expect(count.peek()).toBe(2);
    dispose();
  });
});

test("effect 的 apply 可以写其他 cell", () => {
  createRoot((dispose) => {
    const source = createCell(1);
    const mirror = createCell(0);
    createEffect(
      () => source(),
      (value) => {
        mirror.set(value * 10);
      },
    );
    flush();
    source.set(3);
    flush();
    expect(mirror.peek()).toBe(30);
    dispose();
  });
});

test("createValueMemo：按值相等时不通知下游", () => {
  createRoot((dispose) => {
    const query = createCell({ page: 1, size: 10 });
    const stable = createValueMemo(() => ({ ...query() }));
    let downstream = 0;
    createEffect(
      () => stable(),
      () => {
        downstream += 1;
      },
    );
    flush();
    query.set({ page: 1, size: 10 });
    flush();
    expect(downstream).toBe(1);
    query.set({ page: 2, size: 10 });
    flush();
    expect(downstream).toBe(2);
    dispose();
  });
});

test("structuralEquals：数组、对象、Map、Set 与 Date 按值比较", () => {
  expect(structuralEquals({ a: [1, { b: null }] }, { a: [1, { b: null }] })).toBe(true);
  expect(structuralEquals({ a: 1 }, { a: 1, b: undefined })).toBe(false);
  expect(structuralEquals([1, 2], [2, 1])).toBe(false);
  expect(structuralEquals(new Map([["k", [1]]]), new Map([["k", [1]]]))).toBe(true);
  expect(structuralEquals(new Set([1, 2]), new Set([2, 1]))).toBe(true);
  expect(structuralEquals(new Date(5), new Date(5))).toBe(true);
  expect(structuralEquals(Number.NaN, Number.NaN)).toBe(true);
  expect(structuralEquals([], {})).toBe(false);
});

test("createTaskResource：依赖变化取消旧任务并保留旧值，支持重启、取消与清空", async () => {
  const log: string[] = [];
  let dispose = () => {};
  const { page, resource } = createRoot((disposeRoot) => {
    dispose = disposeRoot;
    const pageCell = createCell(1);
    const taskResource = createTaskResource(
      () => pageCell(),
      async (current, context) => {
        await sleep(15);
        log.push(`done:${current}:${context.alive()}`);
        return `page-${current}`;
      },
    );
    return { page: pageCell, resource: taskResource };
  });
  flush();
  expect(resource.state()).toBe("pending");
  expect(resource()).toBeUndefined();
  await sleep(40);
  expect(resource.state()).toBe("ready");
  expect(resource.peek()).toBe("page-1");

  page.set(2);
  flush();
  expect(resource.state()).toBe("pending");
  expect(resource.peek()).toBe("page-1");
  page.set(3);
  flush();
  await sleep(50);
  expect(resource.peek()).toBe("page-3");
  expect(log).toContain("done:2:false");

  resource.restart();
  expect(resource.state()).toBe("pending");
  resource.cancel();
  await sleep(40);
  expect(resource.state()).toBe("stopped");
  expect(resource.peek()).toBe("page-3");
  resource.clear();
  expect(resource.peek()).toBeUndefined();

  page.set(9);
  flush();
  dispose();
  await sleep(40);
  expect(resource.peek()).toBeUndefined();
});

test("createTaskResource：首次提交前的重启只用初始依赖启动一次", () => {
  createRoot((dispose) => {
    try {
      const seen: number[] = [];
      const page = createCell(1);
      const resource = createTaskResource(page, async (current) => {
        seen.push(current);
        return current;
      });
      resource.restart();
      flush();
      expect(seen).toEqual([1]);
    } finally {
      dispose();
    }
  });
});

test("createTaskResource：同轮更新依赖与多次重启只启动最新任务", () => {
  createRoot((dispose) => {
    try {
      const seen: number[] = [];
      const page = createCell(1);
      const resource = createTaskResource(page, async (current) => {
        seen.push(current);
        return current;
      });
      flush();
      page.set(2);
      resource.restart();
      resource.restart();
      flush();
      expect(seen).toEqual([1, 2]);
      resource.restart();
      flush();
      expect(seen).toEqual([1, 2, 2]);
    } finally {
      dispose();
    }
  });
});

test.each([true, false])(
  "createTaskResource：跨 effect 更新后只启动最新任务，手动重启 %s",
  (restart) => {
    createRoot((dispose) => {
      try {
        const seen: number[] = [];
        const page = createCell(1);
        createEffect(page, (current) => {
          if (current === 2) {
            page.set(3);
            if (restart) {
              resource.restart();
            }
          }
        });
        const pageQuery = createValueMemo(() => page());
        const resource = createTaskResource(pageQuery, async (current) => {
          seen.push(current);
          return current;
        });
        flush();
        page.set(2);
        flush();
        expect(seen).toEqual([1, 3]);
      } finally {
        dispose();
      }
    });
  },
);

test("createTaskResource：取消待提交的重启后仍响应新的依赖变化", () => {
  createRoot((dispose) => {
    try {
      const seen: number[] = [];
      const page = createCell(1);
      const resource = createTaskResource(page, async (current) => {
        seen.push(current);
        return current;
      });
      flush();
      resource.restart();
      resource.cancel();
      flush();
      expect(resource.state()).toBe("stopped");
      expect(seen).toEqual([1]);
      page.set(2);
      flush();
      expect(seen).toEqual([1, 2]);
    } finally {
      dispose();
    }
  });
});

test("createTaskScope：宿主销毁后任务不再存活，新任务不执行", async () => {
  let dispose = () => {};
  const scope = createRoot((disposeRoot) => {
    dispose = disposeRoot;
    return createTaskScope();
  });
  const result = createCell("none");
  const aborted: boolean[] = [];
  scope.spawn(async (context) => {
    await sleep(15);
    aborted.push(context.signal.aborted);
    if (!context.alive()) {
      return;
    }
    result.set("written");
  });
  dispose();
  await sleep(40);
  expect(result.peek()).toBe("none");
  expect(aborted).toEqual([true]);
  expect(scope.alive()).toBe(false);

  let ran = false;
  const late = scope.spawn(async () => {
    ran = true;
  });
  expect(late.finished()).toBe(true);
  expect(ran).toBe(false);
});

test("任务句柄：单独取消一个任务不影响同宿主的其他任务", async () => {
  let dispose = () => {};
  const scope = createRoot((disposeRoot) => {
    dispose = disposeRoot;
    return createTaskScope();
  });
  const seen: string[] = [];
  const first = scope.spawn(async (context) => {
    await sleep(15);
    seen.push(`first:${context.alive()}`);
  });
  scope.spawn(async (context) => {
    await sleep(15);
    seen.push(`second:${context.alive()}`);
  });
  first.cancel();
  expect(first.finished()).toBe(true);
  await sleep(40);
  expect(seen.sort()).toEqual(["first:false", "second:true"]);
  dispose();
});
