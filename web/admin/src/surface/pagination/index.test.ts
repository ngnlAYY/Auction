// Admin 分页数量切换的三步写入：先固定旧数量，再 push 首页，最后 replace 新数量。

import { expect, test } from "bun:test";
import { type QueryPair, useQueryState } from "@auction/web-shared/net/query";
import { createCell } from "@auction/web-shared/reactive/cell";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { Pager } from "@auction/web-shared/ui/components/pager";
import { PageSizePicker } from "@auction/web-shared/ui/components/pager/size";
import { createRoot, flush } from "solid-js";
import { installAdminTranslation } from "../admin-locale-fixture.test";

interface Element {
  readonly tag: unknown;
  readonly props: Record<string, unknown>;
}

/** 捕获 JSX 节点，并展开分页器的两个函数组件，让真实 `<select>` 处理器可见。 */
function capturePagerJsx(elements: Element[]) {
  return (tag: unknown, props: Record<string, unknown> | null, ...children: unknown[]): unknown => {
    const element = { tag, props: props ?? {}, children };
    elements.push(element);
    if (typeof tag === "function" && (tag === Pager || tag === PageSizePicker)) {
      return Reflect.apply(tag, undefined, [element.props]);
    }
    return element;
  };
}

/** 取出数量选择器的真实 change 处理器；没有经过 `<select>` 的接线就算夹具失败。 */
function pageSizeChange(
  elements: readonly Element[],
): (event: { currentTarget: { value: string } }) => void {
  const select = elements.find(({ tag, props }) => tag === "select" && props.name === "page_size");
  const onChange = select?.props.onChange;
  if (typeof onChange !== "function") {
    throw new Error("未挂载每页数量选择器");
  }
  return (event) => {
    onChange(event);
  };
}

async function withPagerFixture(
  path: string,
  run: (fixture: {
    fake: ReturnType<typeof installFakeWindow>["fake"];
    elements: readonly Element[];
    mount<T>(setup: () => T): T;
  }) => Promise<void>,
): Promise<void> {
  const previousJsx = Object.getOwnPropertyDescriptor(globalThis, "jsx");
  const elements: Element[] = [];
  const { fake, restore } = installFakeWindow(path);
  let dispose = (): void => {};
  try {
    const restoreTranslation = installAdminTranslation((id) => id);
    try {
      Object.defineProperty(globalThis, "jsx", {
        configurable: true,
        value: capturePagerJsx(elements),
      });
      try {
        await run({
          fake,
          elements,
          mount(setup) {
            return createRoot((disposeRoot) => {
              dispose = disposeRoot;
              return setup();
            });
          },
        });
      } finally {
        try {
          dispose();
          flush();
        } finally {
          if (previousJsx === undefined) Reflect.deleteProperty(globalThis, "jsx");
          else Object.defineProperty(globalThis, "jsx", previousJsx);
        }
      }
    } finally {
      restoreTranslation();
    }
  } finally {
    restore();
  }
}

test("每页数量从 50 切到 100 后，后退仍回到每页 50 的条目", async () => {
  await withPagerFixture("/admin/lots?page_size=50", async ({ fake, elements, mount }) => {
    const { AdminPager, useAdminPagination } = await import(".");
    const { page, pagination } = mount(() => {
      const pagination = useAdminPagination("lots", ["page_size"], 100, 100_000);
      const page = useQueryState<number>(
        ["page"],
        (pairs: readonly QueryPair[]) => {
          const value = pairs.find(([key]) => key === "page")?.[1];
          return value === undefined ? 1 : Number(value);
        },
        (page) => (page > 1 ? [["page", page.toString()] as const] : []),
        { sameAddress: "write" },
      );
      AdminPager({
        pagination,
        page: 1,
        pageSize: 50,
        label: "lots",
        onPageChange: (nextPage) => page.push(nextPage),
      });
      flush();
      return { page, pagination };
    });
    const onChange = pageSizeChange(elements);
    expect(pagination.size()).toBe(50);

    const select = { value: "100" };
    onChange({ currentTarget: select });
    flush();
    expect(select.value).toBe("100");
    expect(fake.location.search).toBe("?page_size=100");
    expect(pagination.size()).toBe(100);
    expect(page.state.peek()).toBe(1);

    fake.history.back();
    flush();
    expect(fake.location.search).toBe("?page_size=50");
    expect(pagination.size()).toBe(50);
    expect(page.state.peek()).toBe(1);
    expect(elements.length).toBeGreaterThan(0);
  });
});

test("size_reset_uses_the_latest_callback_and_defaults_to_first_page：数量重置使用最新回调并回到第一页", async () => {
  await withPagerFixture("/admin/lots", async ({ elements, mount }) => {
    const { AdminPager, useAdminPagination } = await import(".");
    const calls: Array<readonly [number, number]> = [];
    const updateRevision = mount(() => {
      const revision = createCell(0);
      const pagination = useAdminPagination("test", ["page_size"], null, 10_000);
      const props: Parameters<typeof AdminPager>[0] = {
        pagination,
        page: 1,
        get pageSize() {
          return pagination.size();
        },
        get label() {
          return `分页 ${revision()}`;
        },
        total: 100,
        onPageChange(page) {
          calls.push([revision() === 0 ? 0 : -revision(), page]);
        },
      };
      // 按 Rust 的 props 构造方式真正增删可选回调，而不是安装一个返回 false 的替身。
      AdminPager(props);
      flush();
      return (value: number) => {
        revision.set(value);
        if (value >= 1 && value <= 3) {
          expect(
            Reflect.set(props, "onSizeReset", () => {
              calls.push([value, 1]);
              return true;
            }),
          ).toBe(true);
        } else {
          expect(Reflect.deleteProperty(props, "onSizeReset")).toBe(true);
        }
      };
    });
    const onChange = pageSizeChange(elements);

    onChange({ currentTarget: { value: "20" } });
    flush();
    expect(calls).toEqual([[0, 1]]);

    for (const revision of [1, 2, 3]) {
      updateRevision(revision);
      flush();
    }
    onChange({ currentTarget: { value: "50" } });
    flush();
    expect(calls).toEqual([
      [0, 1],
      [3, 1],
    ]);

    updateRevision(4);
    flush();
    onChange({ currentTarget: { value: "100" } });
    flush();
    expect(calls).toEqual([
      [0, 1],
      [3, 1],
      [-4, 1],
    ]);
  });
});
