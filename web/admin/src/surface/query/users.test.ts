// 用户查询写入：同址提交也创建历史项，写入失败时保留已发布状态。

import { expect, test } from "bun:test";
import type { QueryState } from "@auction/web-shared/net/query";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { createRoot, flush } from "solid-js";

import {
  type UsersQuery,
  usersQueryApply,
  usersQueryCorrectPage,
  useUsersQueryState,
} from "./users";

function mountUsersQuery(initialUrl: string) {
  const { fake, restore } = installFakeWindow(initialUrl);
  let state: QueryState<UsersQuery> | undefined;
  let dispose = (): void => {};
  createRoot((disposeRoot) => {
    dispose = disposeRoot;
    state = useUsersQueryState();
    flush();
  });
  return {
    fake,
    state() {
      if (state === undefined) throw new Error("未装配用户查询");
      return state;
    },
    dispose() {
      dispose();
      flush();
      restore();
    },
  };
}

test("连续两次提交同一筛选各创建一条历史项", () => {
  const mounted = mountUsersQuery("/admin/users?name=alice");
  try {
    const state = mounted.state();
    const query = state.state.peek();
    const pushes = mounted.fake.writes.filter(({ mode }) => mode === "push").length;
    const firstKey = (mounted.fake.history.state as unknown[] | null)?.[2];

    usersQueryApply(query, state);
    const afterFirst = (mounted.fake.history.state as unknown[])[2];
    usersQueryApply(query, state);
    const afterSecond = (mounted.fake.history.state as unknown[])[2];

    expect(mounted.fake.writes.filter(({ mode }) => mode === "push").length).toBe(pushes + 2);
    expect(afterFirst).not.toBe(firstKey);
    expect(afterSecond).not.toBe(afterFirst);
    expect(mounted.fake.location.search).toBe("?name=alice");
    expect(state.state.peek()).toEqual(query);
  } finally {
    mounted.dispose();
  }
});

test("浏览器拒绝 push 时保留原查询与地址", () => {
  const mounted = mountUsersQuery("/admin/users?name=alice");
  try {
    const state = mounted.state();
    const before = state.state.peek();
    const href = mounted.fake.location.href;
    const pushes = mounted.fake.writes.filter(({ mode }) => mode === "push").length;
    mounted.fake.rejectWrites("push");

    usersQueryApply({ ...before, name: "bob", page: 2n }, state);

    expect(state.state.peek()).toEqual(before);
    expect(mounted.fake.location.href).toBe(href);
    expect(mounted.fake.writes.filter(({ mode }) => mode === "push").length).toBe(pushes);
  } finally {
    mounted.dispose();
  }
});

test("纠偏后的地址与当前地址相同时，同址 replace 视为成功", () => {
  const mounted = mountUsersQuery("/admin/users?name=alice");
  try {
    const state = mounted.state();
    const replaces = mounted.fake.writes.filter(({ mode }) => mode === "replace").length;
    const query = { ...state.state.peek(), page: 2n };

    expect(usersQueryCorrectPage(query, state, { page: 2, page_size: 10, total: 5 })).toBe(true);
    expect(mounted.fake.location.search).toBe("?name=alice");
    expect(mounted.fake.writes.filter(({ mode }) => mode === "replace").length).toBe(replaces + 1);
    expect(state.state.peek().page).toBe(1n);
  } finally {
    mounted.dispose();
  }
});
