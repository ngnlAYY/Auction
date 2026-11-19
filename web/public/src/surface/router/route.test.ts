// 登录与状态路由的首次挂载和历史遍历。

import { expect, test } from "bun:test";
import { hasClass, mountCompetition, waitFor } from "../commands/competition-fixture.test";

const STATUS = { path: "/status", className: "public-status-page" };
const LOGIN = { path: "/login", className: "public-account-login-form" };

test.each([
  [STATUS, LOGIN],
  [LOGIN, STATUS],
] as const)("从 $path 直达后可切页并返回", async (initial, next) => {
  const page = await mountCompetition("main", false, { href: initial.path });
  const find = ({ className }: typeof STATUS) =>
    page.find((element) => hasClass(element, className));

  try {
    await waitFor(() => find(initial) !== null);
    expect(find(next)).toBeNull();

    page.go(next.path);
    await waitFor(() => find(next) !== null);
    expect(find(initial)).toBeNull();

    page.back();
    await waitFor(() => find(initial) !== null);
    expect(page.location.pathname).toBe(initial.path);
    expect(find(next)).toBeNull();
  } finally {
    page.dispose();
  }
});
