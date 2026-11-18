// 目录列设置离开焦点后的 Escape 交互。

import { expect, test } from "bun:test";
import { flush } from "solid-js";
import {
  dispatch,
  hasClass,
  mountCompetition,
  waitFor,
} from "../../commands/competition-fixture.test";
import { checkPagePanelEscape, installDialogFocus } from "../../shell/dialog-focus-fixture.test";

test("列设置在焦点移出后按 Escape 收起并还焦", () =>
  checkPagePanelEscape("public-catalog-columns-toggle"));

test.each(["账户入口", "列设置面板", "面板外"] as const)(
  "两个面板共存时从%s按 Escape，只收起对应面板",
  async (position) => {
    const dom = installDialogFocus();
    const page = await mountCompetition();
    try {
      dom.attach(page.find);
      const columnsId = "public-catalog-columns-toggle";
      const accountId = "public-account-session-trigger";
      await waitFor(() =>
        [columnsId, accountId].every((id) => page.find((node) => node.getAttribute("id") === id)),
      );
      const byId = (id: string) => page.need((node) => node.getAttribute("id") === id);
      const columns = byId(columnsId);
      const account = byId(accountId);
      dispatch(columns, "Click");
      dom.focus(account);
      dispatch(account, "Click");
      expect(columns.getAttribute("aria-expanded")).toBe("true");
      expect(account.getAttribute("aria-expanded")).toBe("true");

      const focus =
        position === "账户入口"
          ? account
          : position === "列设置面板"
            ? byId("public-catalog-column-all")
            : page.need((node) => hasClass(node, "public-connection-refresh"));
      dom.focus(focus);
      expect(dom.keydown(focus, "Escape")).toBe(true);
      flush();
      const closesColumns = position === "列设置面板";
      const [closed, remaining] = closesColumns ? [columns, account] : [account, columns];
      expect(closed.getAttribute("aria-expanded")).toBe("false");
      expect(remaining.getAttribute("aria-expanded")).toBe("true");
      expect(dom.activeElement()).toBe(closed);
      expect(dom.keydown(closed, "Escape")).toBe(true);
      flush();
      expect(remaining.getAttribute("aria-expanded")).toBe("false");
      expect(dom.activeElement()).toBe(remaining);
    } finally {
      page.dispose();
      dom.restore();
    }
  },
);
