// 竞价恢复的导航、会话和活动行回归。

import { expect, jest, test } from "bun:test";
import { setImmediate } from "node:timers";
import { DEFAULT_REQUEST_TIMEOUT_MILLIS } from "@auction/web-shared/net/transport";
import { t } from "@auction/web-shared/ui/i18n";
import { flush } from "solid-js";
import { installDialogFocus } from "../shell/dialog-focus-fixture.test";
import type { DomElement } from "../shell/navigation-fixture.test";
import { reject } from "../shell/session-fixture";
import {
  bidReceipt,
  type CompetitionPage,
  confirm,
  dispatch,
  hasClass,
  LOT_ID,
  mountCompetition,
  openBid,
  renderedText,
  waitFor,
} from "./competition-fixture.test";

async function settleImmediately() {
  await new Promise<void>((resolve) => setImmediate(resolve));
  flush();
}

const NOTICE = "public-command-notices";
const ENTRY_STATUS = "public-command-notices__entry-status";
const RECEIPT = "public-command-notices__receipt";

const boardRoot = (element: DomElement) => hasClass(element, "public-auction-page");

function notice(page: CompetitionPage): DomElement {
  return page.need((element) => hasClass(element, NOTICE));
}

function button(page: CompetitionPage, key: string, parent: DomElement): DomElement {
  return page.need(
    (element) => element.tag === "button" && renderedText(element) === t(key),
    parent,
  );
}

function bidForm(page: CompetitionPage): DomElement {
  return page.need(
    (element) => element.tag === "form" && hasClass(element, "public-auction-bid-form"),
  );
}

test("在途竞价经过兜底路由时看板卸载，外壳照常收取回执", async () => {
  const page = await mountCompetition();
  try {
    await openBid(page);
    const first = await confirm(page, bidForm(page));
    const board = page.need(boardRoot);
    const shell = page.need((element) => element.attributes.id === "public-content");
    page.go("/unmatched-command-route");
    await waitFor(
      () => page.location.pathname === "/" && page.find((element) => element === board) === null,
    );
    expect(page.find((element) => element === shell)).toBe(shell);
    expect(first.signal.aborted).toBe(false);

    first.respond(bidReceipt());
    await waitFor(() => page.find((element) => hasClass(element, RECEIPT)) !== null);
    expect(renderedText(page.need((element) => hasClass(element, RECEIPT)))).toContain(
      t("bid-success"),
    );
    expect(page.find((element) => hasClass(element, NOTICE))).toBeNull();
  } finally {
    page.dispose();
  }
});

test("活动行确认放弃后恢复当前资格，可以立即提交新出价", async () => {
  const page = await mountCompetition();
  try {
    const original = await openBid(page);
    const first = await confirm(page, bidForm(page));
    first.respond(reject("storage_unavailable", 503));
    await waitFor(() => page.find((element) => hasClass(element, NOTICE)) !== null);
    const close = () =>
      dispatch(
        page.need((element) => hasClass(element, "ui-dialog__close")),
        "Click",
      );
    close();
    const recovery = page.need(
      (element) =>
        element.tag === "button" &&
        (element === original ||
          element.attributes["aria-label"] === t("command-recovery-open", { code: "Clo01" })),
    );
    dispatch(recovery, "Click");
    const dialogEntry = page
      .need((element) => element.attributes.id === `public-command-dialog-${LOT_ID}`)
      .getAttribute("id");
    expect(dialogEntry).toBe(`public-command-dialog-${LOT_ID}`);
    const dialog = page.need((element) => element.attributes.role === "dialog");
    dispatch(button(page, "command-abandon", dialog), "Click");
    const confirmation = page.need((element) =>
      hasClass(element, "public-command-notices__confirmation"),
    );
    dispatch(button(page, "command-abandon", confirmation), "Click");

    const form = bidForm(page);
    const submit = page.need(
      (element) => element.tag === "button" && element.attributes.type === "submit",
      form,
    );
    expect(submit.attributes.disabled).toBe(false);
    expect(recovery).toBe(original);
    expect(page.find((element) => element === original)).toBe(original);
    const next = await confirm(page, form);
    expect(next.body).not.toBe(first.body);
    expect(next.path).toBe(first.path);
    next.respond(bidReceipt());
  } finally {
    page.dispose();
  }
});

test.each([false, true])("关闭在途更新过的竞价弹窗后恢复焦点（原行卸载 %s）", async (removed) => {
  const focus = installDialogFocus();
  try {
    const page = await mountCompetition();
    try {
      focus.attach(page.find);
      const bid = (element: DomElement) =>
        element.tag === "button" &&
        element.attributes["aria-label"] === t("catalog-bid-lot", { code: "Clo01" });
      await waitFor(() => page.find(bid) !== null);
      const opener = page.need(bid);
      focus.captureClick(opener);
      dispatch(opener, "Click");
      focus.commit();
      expect(focus.activeElement()).not.toBeNull();
      expect(focus.activeElement()).not.toBe(opener);
      const first = await confirm(page, bidForm(page));
      focus.commit();
      page.server.lotEnded = removed;
      first.respond(reject("storage_unavailable", 503));
      await waitFor(() => page.find((element) => hasClass(element, NOTICE)) !== null);
      if (removed) {
        await waitFor(() => page.find((element) => element === opener) === null);
      } else {
        expect(page.find((element) => element === opener)).toBe(opener);
      }
      const close = page.need((element) => hasClass(element, "ui-dialog__close"));
      focus.captureClick(close);
      dispatch(close, "Click");
      focus.commit();
      expect(page.find((element) => element.attributes.role === "dialog")).toBeNull();
      expect(focus.activeElement()).toBe(
        removed ? page.need((element) => element.attributes.id === "public-content") : opener,
      );
    } finally {
      page.dispose();
    }
  } finally {
    focus.restore();
  }
});

test("401 响应头即启动复核，正文超时后继续暂停未知命令的重试", async () => {
  const page = await mountCompetition();
  try {
    await openBid(page);
    const original = await confirm(page, bidForm(page));
    original.respond(reject("storage_unavailable", 503));
    await waitFor(() => page.find((element) => hasClass(element, NOTICE)) !== null);
    page.go("/login");
    page.server.readSession = () => reject("storage_unavailable", 503);
    const readsBefore = page.reads.filter((url) => url === "/api/v1/session").length;
    jest.useFakeTimers();
    dispatch(button(page, "data-retry", notice(page)), "Click");
    await settleImmediately();
    const replay = page.commands[1];
    if (replay === undefined) throw new Error("未重放原命令");
    expect(replay.body).toBe(original.body);
    let reading = false;
    const response = new Response(null, { status: 401 });
    response.arrayBuffer = () => {
      reading = true;
      return new Promise<ArrayBuffer>((_resolve, reject) => {
        replay.signal.addEventListener("abort", () => reject(replay.signal.reason), { once: true });
      });
    };

    replay.respond(response);
    await settleImmediately();
    expect(reading).toBe(true);
    const readsWhileBodyPending = page.reads.filter((url) => url === "/api/v1/session").length;
    jest.advanceTimersByTime(DEFAULT_REQUEST_TIMEOUT_MILLIS);
    await settleImmediately();
    const status = page.need((element) => hasClass(element, ENTRY_STATUS), notice(page));
    expect(renderedText(status)).toContain(t("error-request-timeout"));
    expect(button(page, "data-retry", notice(page)).attributes.disabled).toBe(true);
    expect(readsWhileBodyPending).toBeGreaterThan(readsBefore);
    expect(renderedText(status)).toContain(t("command-retry-session-pending"));
    expect(page.commands).toHaveLength(2);
  } finally {
    jest.useRealTimers();
    page.dispose();
  }
});
