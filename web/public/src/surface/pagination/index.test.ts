// Public 分页数量的接受范围：预设与默认值停在契约上限。

import { expect, spyOn, test } from "bun:test";
import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";
import { createCell } from "@auction/web-shared/reactive/cell";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { Pager } from "@auction/web-shared/ui/components/pager";
import { PageSizePicker } from "@auction/web-shared/ui/components/pager/size";
import { flush } from "solid-js";

import { mountPublicPagerForDiagnostics } from "../shell/navigation-fixture.test";
import { mountReadPage } from "./browser-fixture";

const {
  acceptedPublicPageSize,
  commitPublicPageSizeChoice,
  DEFAULT_PAGE_SIZE,
  normalizePublicPageSize,
  PublicPager,
  PUBLIC_PAGE_SIZES,
  updatePublicPageSize,
  updatePublicPage,
} = await import(".");
const { publicBoardQueryDefault, usePublicQueryState } = await import("../query");

function installPageSizeWindow(initialUrl: string, initialStorage: Map<string, string>) {
  const { fake, restore } = installFakeWindow(initialUrl);
  return {
    fake: Object.assign(fake, {
      localStorage: {
        getItem: (key: string) => initialStorage.get(key) ?? null,
        setItem: (key: string, value: string) => initialStorage.set(key, value),
      },
      storageValue: (key: string) => initialStorage.get(key) ?? null,
    }),
    restore,
  };
}

test("page_size_choices_stop_at_the_contract_maximum：每页数量选择停在契约上限", () => {
  expect(MAX_PUBLIC_PAGE_SIZE).toBe(200);
  for (const size of [1, 37, 200, ...PUBLIC_PAGE_SIZES]) {
    expect(acceptedPublicPageSize(size)).toBe(size);
    expect(normalizePublicPageSize(size)).toBe(size);
  }
  // 自定义输入 201 等越界数量不被接受：分页器不写 URL、不保存偏好，也就不会发出请求。
  for (const size of [201, 1_000, 2 ** 63 - 1, 0, -1, -(2 ** 63)]) {
    expect(acceptedPublicPageSize(size)).toBeNull();
    // 其他入口拿到越界值时回退默认值，不截断成上限。
    expect(normalizePublicPageSize(size)).toBe(DEFAULT_PAGE_SIZE);
  }
});

test("failed_page_size_write_keeps_preference：页长写入失败时不保存本地偏好", () => {
  const storage = new Map([
    ["hv.public.page-size", "20"],
    ["hv.public.movement-page-size", "20"],
  ]);
  const mounted = installPageSizeWindow("/", storage);
  try {
    const queryState = createCell(publicBoardQueryDefault());
    expect(queryState.peek().pageSize).toBe(20);

    mounted.fake.rejectWrites(true);
    expect(
      commitPublicPageSizeChoice(50, false, (size) => updatePublicPageSize(queryState, size)),
    ).toBe(false);
    expect(queryState.peek().pageSize).toBe(20);
    expect(mounted.fake.storageValue("hv.public.page-size")).toBe("20");

    mounted.fake.rejectWrites(false);
    expect(
      commitPublicPageSizeChoice(50, false, (size) => updatePublicPageSize(queryState, size)),
    ).toBe(true);
    expect(queryState.peek().pageSize).toBe(50);
    expect(mounted.fake.storageValue("hv.public.page-size")).toBe("50");
    expect(new URLSearchParams(mounted.fake.location.search).get("page_size")).toBe("50");
  } finally {
    mounted.restore();
  }
});

test.each([false, true])(
  "旧页面回调不返回布尔值时核对地址与页长偏好（移动明细 %s）",
  (movement) => {
    const key = movement ? "hv.public.movement-page-size" : "hv.public.page-size";
    const sizeParam = movement ? "movement_page_size" : "page_size";
    const mounted = installPageSizeWindow(
      `/compensation-pool?${sizeParam}=20`,
      new Map([[key, "20"]]),
    );
    const write = (size: number) => {
      try {
        mounted.fake.history.pushState(null, "", `/compensation-pool?${sizeParam}=${size}`);
      } catch {
        // 旧页面从更新函数丢弃失败结果，调用方须以地址栏为准。
      }
    };
    try {
      mounted.fake.rejectWrites(true);
      expect(commitPublicPageSizeChoice(50, movement, write)).toBe(false);
      expect(mounted.fake.storageValue(key)).toBe("20");
      mounted.fake.rejectWrites(false);
      expect(commitPublicPageSizeChoice(50, movement, write)).toBe(true);
      expect(mounted.fake.storageValue(key)).toBe("50");
    } finally {
      mounted.restore();
    }
  },
);

test("地址里的页长恰好相同不能证明旧回调成功重置页码", () => {
  const storage = new Map([["hv.public.page-size", "50"]]);
  const mounted = installPageSizeWindow("/compensation-pool?page_size=20&page=3", storage);
  try {
    expect(commitPublicPageSizeChoice(20, false, () => {})).toBe(false);
    expect(mounted.fake.storageValue("hv.public.page-size")).toBe("50");
  } finally {
    mounted.restore();
  }
});

test("分页器的跳页被拒后恢复输入，冻结旧页长成功但提交新页长被拒时恢复下拉框", () => {
  const probe: { state?: ReturnType<typeof usePublicQueryState>["state"] } = {};
  const mounted = mountReadPage(
    "/auctions?page=3&extra=%2f",
    () => {
      const query = usePublicQueryState().state;
      probe.state = query;
      PublicPager({
        page: query().page,
        pageSize: query().pageSize,
        total: 120,
        label: "分页",
        onPageChange: (nextPage) => updatePublicPage(query, nextPage, "push"),
        onPageSizeChange: (size) => updatePublicPageSize(query, size),
      });
    },
    ({ tag, props }) => {
      if (typeof tag === "function" && (tag === Pager || tag === PageSizePicker)) {
        Reflect.apply(tag, undefined, [props]);
      }
    },
  );
  try {
    const page = mounted.elements.find(
      ({ tag, props }) => tag === "input" && props.name === "page",
    )?.props;
    const submit = mounted.elements.find(
      ({ tag, props }) => tag === "form" && props.class === "ui-pager__jump",
    )?.props.onSubmit;
    const change = mounted.elements.find(
      ({ tag, props }) => tag === "select" && props.name === "page_size",
    )?.props.onChange;
    if (
      typeof page?.onInput !== "function" ||
      typeof page.ref !== "function" ||
      typeof submit !== "function" ||
      typeof change !== "function"
    ) {
      throw new Error("未挂载分页控件");
    }
    mounted.storage.set("hv.public.page-size", "20");
    mounted.rejectWrites("push");
    const pageInput = { value: "4" };
    page.ref(pageInput);
    page.onInput({ currentTarget: pageInput });
    flush();
    submit({ preventDefault() {} });
    expect(pageInput.value).toBe("3");

    const before = mounted.writes.length;
    const select = { value: "50" };
    change({ currentTarget: select });
    flush();
    expect(mounted.writes.slice(before).map(({ mode }) => mode)).toEqual(["replace"]);
    expect(mounted.location.search).toBe("?page=3&extra=%2f&page_size=20&movement_page_size=20");
    expect(probe.state?.peek()).toMatchObject({ page: 3, pageSize: 20 });
    expect(select.value).toBe("20");
    expect(mounted.storage.get("hv.public.page-size")).toBe("20");

    mounted.rejectWrites(false);
    select.value = "50";
    change({ currentTarget: select });
    flush();
    expect(probe.state?.peek()).toMatchObject({ page: 1, pageSize: 50 });
    expect(mounted.location.searchParams.has("page")).toBe(false);
    expect(select.value).toBe("50");
    expect(mounted.storage.get("hv.public.page-size")).toBe("50");
  } finally {
    mounted.dispose();
  }
});

test.each([2, 120])("总数已缩减或随后缩减时，拒绝纠偏保留第 3 页（初始总数 %i）", (total) => {
  const count = createCell(total);
  const attempted: number[] = [];
  const submitted: number[] = [];
  let canWrite = false;
  let tree: unknown;
  const mounted = mountReadPage("/auctions?page=3", () => {
    const query = usePublicQueryState().state;
    tree = Pager({
      get page() {
        return query().page;
      },
      get total() {
        return count();
      },
      pageSize: 10,
      label: "分页",
      previousLabel: "上一页",
      nextLabel: "下一页",
      jumpLabel: "跳页",
      jumpButtonLabel: "前往",
      status: ([page, pages]) => `${page}/${pages}`,
      clampFailure: { message: "页码更新失败", retryLabel: "重试调整页码" },
      onPageChange: (page) => {
        submitted.push(page);
        return updatePublicPage(query, page, "push");
      },
      onPageClamp: (page) => {
        attempted.push(page);
        return canWrite && updatePublicPage(query, page, "replace");
      },
    });
  });
  try {
    const input = mounted.elements.find(
      ({ tag, props }) => tag === "input" && props.name === "page",
    )?.props;
    const submit = mounted.elements.find(
      ({ tag, props }) => tag === "form" && props.class === "ui-pager__jump",
    )?.props.onSubmit;
    if (typeof input?.ref !== "function" || typeof submit !== "function") {
      throw new Error("未挂载跳页控件");
    }
    expect(input.value).toBe("3");
    const nativeInput = { value: String(input.value) };
    input.ref(nativeInput);
    if (total === 2) {
      expect(input.max).toBe("1");
      expect(
        mounted.elements.find(({ props }) => props.class === "ui-pager__status")?.children,
      ).toEqual(["3/1"]);
    }

    count.set(2);
    flush();
    expect(attempted).toEqual([1]);
    expect(nativeInput.value).toBe("3");
    expect(mounted.location.searchParams.get("page")).toBe("3");
    submit({ preventDefault() {} });
    expect(submitted).toEqual([]);

    const retry = mounted
      .visible(tree)
      .find(({ tag, children }) => tag === "button" && children.includes("重试调整页码"))
      ?.props.onClick;
    if (typeof retry !== "function") throw new Error("未挂载纠偏重试");
    canWrite = true;
    retry();
    flush();
    expect(attempted).toEqual([1, 1]);
    expect(mounted.location.searchParams.has("page")).toBe(false);
    expect(mounted.writes.at(-1)?.mode).toBe("replace");
    expect(nativeInput.value).toBe("1");
  } finally {
    mounted.dispose();
  }
});

test("开发模式下 Public 分页输入随页码和页长更新且不报非跟踪读取", async () => {
  const page = createCell(2);
  const size = createCell(20);
  const warnings = spyOn(console, "warn");
  let dispose = () => {};
  try {
    const mounted = await mountPublicPagerForDiagnostics({
      get page() {
        return page();
      },
      get pageSize() {
        return size();
      },
      total: 200,
      label: "分页",
      onPageChange: (target) => page.set(target),
      onPageSizeChange: (target) => size.set(target),
    });
    dispose = mounted.dispose;
    const value = (name: string) =>
      mounted.find((node) => node.getAttribute("name") === name)?.attributes.value;
    expect([value("page"), value("page_size"), value("custom_page_size")]).toEqual([
      "2",
      "20",
      "20",
    ]);
    page.set(3);
    size.set(50);
    flush();
    expect([value("page"), value("page_size"), value("custom_page_size")]).toEqual([
      "3",
      "50",
      "50",
    ]);
    expect(
      warnings.mock.calls.filter(
        ([message]) => typeof message === "string" && message.startsWith("[STRICT_READ_UNTRACKED]"),
      ),
    ).toEqual([]);
  } finally {
    warnings.mockRestore();
    dispose();
  }
});
