// 顶栏账户入口把导航继承的世界交给实际链接。

import { expect, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { ok } from "@auction/web-shared/result";
import { Link } from "@auction/web-shared/router";
import { children, createRoot, Match, untrack } from "solid-js";

import type { PublicSession } from "../../api/session";
import {
  type PublicSessionResource,
  PublicSessionResourceContext,
  type PublicSessionResult,
} from "../index";
import { type NormalizedWorldQuery, worldScopeHref } from "../query";
import { checkPagePanelEscape } from "../shell/dialog-focus-fixture.test";
import { SessionMenu } from "./menu";

const IDENTITY: PublicSession = {
  authenticated: true,
  user_id: "00000000-0000-4000-8000-000000000001",
  username: "Alice",
  expires_at: null,
  absolute_expires_at: null,
};

function accountMenuLinks(world: NormalizedWorldQuery, identity: PublicSession | null): unknown[] {
  const originalJsx = Object.getOwnPropertyDescriptor(globalThis, "jsx");
  if (originalJsx === undefined) {
    throw new Error("测试 JSX 转译未装配");
  }
  const links: unknown[] = [];
  // 捕获实际 Link 组件收到的 href，按 Match 条件求值账号槽位。
  Object.defineProperty(globalThis, "jsx", {
    ...originalJsx,
    value(tag: unknown, attributes: Record<string, unknown> | null, child: unknown) {
      if (tag === Match && attributes?.when && typeof child === "function") {
        child(() => attributes.when);
      }
      if (tag === Link && attributes?.class === "public-account-session-item") {
        links.push(attributes.href);
      }
      return null;
    },
  });
  try {
    createRoot((dispose) => {
      try {
        const snapshot = createCell<PublicSessionResult | undefined>(ok(identity));
        const session: PublicSessionResource = Object.assign(snapshot, {
          state: (): "ready" => "ready",
          restart() {},
          cancel() {},
          clear: () => snapshot.set(undefined),
        });
        const fixture = PublicSessionResourceContext({
          value: session,
          get children() {
            return untrack(() =>
              SessionMenu({ scopedHref: (path) => worldScopeHref(path, world) }),
            );
          },
        });
        children(() => fixture)();
      } finally {
        dispose();
      }
    });
    return links;
  } finally {
    Object.defineProperty(globalThis, "jsx", originalJsx);
  }
}

test("顶栏登录身份的我的链接继承显式世界，缺省值仍可省略", () => {
  expect(accountMenuLinks("main", IDENTITY)).toEqual(["/me?world=main"]);
  expect(accountMenuLinks("isekai", IDENTITY)).toEqual(["/me?world=isekai"]);
  expect(accountMenuLinks(null, IDENTITY)).toEqual(["/me"]);
  expect(accountMenuLinks("isekai", null)).toEqual([]);
});

test("账户面板在焦点移出后按 Escape 收起并还焦", () =>
  checkPagePanelEscape("public-account-session-trigger"));
