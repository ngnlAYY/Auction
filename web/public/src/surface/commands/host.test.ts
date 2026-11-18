// 竞价命令穿过实际表单、外壳路由和 fetch 的生命周期回归。

import { expect, test } from "bun:test";
import { t } from "@auction/web-shared/ui/i18n";
import { invalidateCookieSync, logoutPublic, requestPublicSessionCheck } from "../../api/session";
import type { DomElement } from "../shell/navigation-fixture.test";
import { ALICE, BOB, brokenBody, reject, success } from "../shell/session-fixture";
import {
  AUCTION_ID,
  AUTHORIZATION_ID,
  bidReceipt,
  type CompetitionPage,
  confirm,
  dispatch,
  hasClass,
  LOT_ID,
  mountCompetition,
  openBid,
  proxyReceipt,
  renderedText,
  SECOND_LOT_ID,
  type SentRequest,
  settle,
  waitFor,
} from "./competition-fixture.test";

const NOTICE = "public-command-notices";
const ENTRY_STATUS = "public-command-notices__entry-status";
const RECEIPT = "public-command-notices__receipt";

const boardRoot = (element: DomElement) => hasClass(element, "public-auction-page");

function notice(page: CompetitionPage): DomElement {
  return page.need((element) => hasClass(element, NOTICE));
}

function findText(page: CompetitionPage, text: string, parent?: DomElement) {
  return page.find((element) => renderedText(element) === text, parent);
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

async function showUnknown(
  page: CompetitionPage,
  response = reject("storage_unavailable", 503),
  code = "Clo01",
) {
  await openBid(page, code);
  const first = await confirm(page, bidForm(page));
  first.respond(response);
  await waitFor(() => page.find((element) => hasClass(element, NOTICE)) !== null);
  return first;
}

function clickRetry(page: CompetitionPage): void {
  dispatch(button(page, "data-retry", notice(page)), "Click");
}

async function retry(page: CompetitionPage, first: SentRequest): Promise<SentRequest> {
  const index = page.commands.length;
  clickRetry(page);
  await waitFor(() => page.commands.length > index);
  const replay = page.commands[index];
  if (replay === undefined) throw new Error("未重放原命令");
  expect(replay.path).toBe(first.path);
  expect(replay.expectedUser).toBe(first.expectedUser);
  expect(new TextEncoder().encode(replay.body)).toEqual(new TextEncoder().encode(first.body));
  return replay;
}

function focusAfterDismiss(page: CompetitionPage, dismiss: DomElement): DomElement | null {
  const documentDescriptor = Object.getOwnPropertyDescriptor(globalThis, "document");
  const elementDescriptor = Object.getOwnPropertyDescriptor(globalThis, "HTMLElement");
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: { getElementById: (id: string) => page.find((element) => element.attributes.id === id) },
  });
  Object.defineProperty(globalThis, "HTMLElement", {
    configurable: true,
    value: dismiss.constructor,
  });
  try {
    dispatch(dismiss, "Click");
    return page.find((element) => element.attributes.focused === true);
  } finally {
    if (documentDescriptor === undefined) Reflect.deleteProperty(globalThis, "document");
    else Object.defineProperty(globalThis, "document", documentDescriptor);
    if (elementDescriptor === undefined) Reflect.deleteProperty(globalThis, "HTMLElement");
    else Object.defineProperty(globalThis, "HTMLElement", elementDescriptor);
  }
}

test("已发出的直接出价在生产看板卸载后照常收包，原样重试成功仅回显一次", async () => {
  const page = await mountCompetition();
  try {
    await openBid(page);
    const form = bidForm(page);
    const first = await confirm(page, form);
    expect(first.body).toContain('"amount":2000');
    expect(first.body).toMatch(/"command_id":"[0-9a-f-]+"/u);
    expect(first.expectedUser).toBe(ALICE.user_id);

    const board = page.need(boardRoot);
    page.go("/login");
    await waitFor(() => page.find((element) => element.attributes.name === "login_key") !== null);
    expect(page.find(boardRoot)).toBeNull();
    expect(page.find((element) => element === board)).toBeNull();
    expect(first.signal.aborted).toBe(false);
    first.respond(brokenBody());
    await waitFor(() => page.find((element) => hasClass(element, NOTICE)) !== null);
    const section = notice(page);
    expect(renderedText(section)).toContain("Legendary Cotton Robe");
    expect(renderedText(section)).toContain(t("command-memory-only"));
    expect(section.getAttribute("aria-labelledby")).toBe("public-command-notices-title");
    expect([...page.storage.values()].join(" ")).not.toContain(
      first.body.match(/[0-9a-f-]{36}/u)?.[0] ?? first.body,
    );

    // 实际 App 的兜底路由重定向首页时外壳及记录保持同一实例。
    page.go("/unknown-audit-route");
    await waitFor(() => page.location.pathname === "/");
    expect(notice(page)).toBe(section);
    page.go("/login");
    const replay = await retry(page, first);
    expect(
      renderedText(page.need((element) => hasClass(element, ENTRY_STATUS), section)),
    ).toContain(t("command-retrying"));
    expect(button(page, "data-retry", section).attributes.disabled).toBe(true);
    replay.respond(bidReceipt());

    await waitFor(() => page.find((element) => hasClass(element, RECEIPT)) !== null);
    expect(page.find((element) => hasClass(element, NOTICE))).toBeNull();
    const result = page.need((element) => hasClass(element, RECEIPT));
    expect(renderedText(result)).toContain(t("bid-success"));
    const live = page.need(
      (element) =>
        hasClass(element, "public-command-notices__status") &&
        renderedText(element).includes(t("bid-success")),
    );
    expect(live.attributes.role).toBe("status");
    expect(live.attributes["aria-atomic"]).toBe("true");
    expect(focusAfterDismiss(page, button(page, "catalog-close", result))?.attributes.id).toBe(
      "public-content",
    );
    expect(page.find((element) => hasClass(element, RECEIPT))).toBeNull();
  } finally {
    page.dispose();
  }
});

test.each(["create", "lower"] as const)(
  "代理 %s 经真实表单提交后跨路由逐字节重放",
  async (kind) => {
    const page = await mountCompetition();
    try {
      page.server.activeProxy = kind === "lower";
      await openBid(page);
      dispatch(
        page.need(
          (element) => element.tag === "button" && renderedText(element) === t("bid-tab-proxy"),
        ),
        "Click",
      );
      const maximumId = `public-proxy-${kind === "lower" ? "lower-" : ""}maximum-${LOT_ID}`;
      await waitFor(() => page.find((element) => element.attributes.id === maximumId) !== null);
      if (kind === "create") {
        dispatch(
          page.need((element) => element.attributes.id === `public-proxy-mode-${LOT_ID}-slow`),
          "Change",
        );
      }
      const form = page.need((element) => element.tag === "form");
      const first = await confirm(page, form);
      expect(first.body).toContain('"maximum_amount":2000');
      if (kind === "create") {
        expect(first.path).toBe(`/api/v1/lots/${LOT_ID}/proxy-authorizations`);
        expect(first.body).toContain('"mode":"slow"');
      } else {
        expect(first.path).toBe(
          `/api/v1/lots/${LOT_ID}/proxy-authorizations/${AUTHORIZATION_ID}/lower`,
        );
      }
      const board = page.need(boardRoot);
      page.go("/login");
      expect(page.find(boardRoot)).toBeNull();
      expect(page.find((element) => element === board)).toBeNull();
      expect(first.signal.aborted).toBe(false);
      first.respond(reject("temporarily_unavailable", 503));
      await waitFor(() => page.find((element) => hasClass(element, NOTICE)) !== null);
      const replay = await retry(page, first);
      replay.respond(proxyReceipt(kind));
      await waitFor(() => page.find((element) => hasClass(element, RECEIPT)) !== null);
      expect(renderedText(page.need((element) => hasClass(element, RECEIPT)))).toContain(
        t("proxy-saved"),
      );
      expect(page.find((element) => hasClass(element, NOTICE))).toBeNull();
    } finally {
      page.dispose();
    }
  },
);

test("同一拍品只显示恢复，结束后可从目录和抽屉打开，其他拍品可以提交", async () => {
  const page = await mountCompetition();
  try {
    const oldTab = await openBid(page);
    const oldForm = bidForm(page);
    const proxyTab = page.need(
      (element) => element.tag === "button" && renderedText(element) === t("bid-tab-proxy"),
    );
    const first = await confirm(page, oldForm);
    page.server.lotEnded = true;
    first.respond(reject("temporarily_unavailable", 503));
    const recovery = (element: DomElement) =>
      element.attributes["aria-label"] === t("command-recovery-open", { code: "Clo01" });
    await waitFor(() => page.find(recovery) !== null);
    expect(page.find((element) => element === oldTab)).toBeNull();
    expect(page.find((element) => element === proxyTab)).toBeNull();
    dispatch(proxyTab, "Click");
    dispatch(oldForm, "Submit");
    await settle();
    expect(page.commands).toHaveLength(1);

    const closeDialog = () =>
      dispatch(
        page.need((element) => hasClass(element, "ui-dialog__close")),
        "Click",
      );
    closeDialog();
    dispatch(page.need(recovery), "Click");
    expect(page.find((element) => element.tag === "form")).toBeNull();
    expect(page.find((element) => hasClass(element, "public-bid-dialog__tabs"))).toBeNull();
    closeDialog();

    dispatch(
      page.need(
        (element) =>
          element.attributes["aria-label"] === t("catalog-view-lot", { code: "Clo01" }) &&
          element.tag === "button",
      ),
      "Click",
    );
    const drawer = page.need((element) => hasClass(element, "public-catalog-detail"));
    expect(renderedText(drawer)).toContain(t("command-lot-unresolved"));
    dispatch(
      page.need((element) => hasClass(element, "public-catalog-detail__bid"), drawer),
      "Click",
    );
    expect(page.find((element) => hasClass(element, "public-bid-dialog__tabs"))).toBeNull();
    closeDialog();
    dispatch(
      page.need((element) => hasClass(element, "public-catalog-detail__close")),
      "Click",
    );

    await openBid(page, "Clo02");
    const other = await confirm(page, bidForm(page));
    expect(other.path).toBe(`/api/v1/lots/${SECOND_LOT_ID}/bids`);
    expect(other.body).not.toBe(first.body);
    other.respond(bidReceipt(SECOND_LOT_ID));
    await settle();
    expect(page.find((element) => hasClass(element, NOTICE))).not.toBeNull();
  } finally {
    page.dispose();
  }
});

test("放弃需二次确认，取消保留记录，确认只移除本地记录", async () => {
  const page = await mountCompetition();
  try {
    await showUnknown(page);
    page.go("/login");
    const section = notice(page);
    const abandon = button(page, "command-abandon", section);
    dispatch(abandon, "Click");
    let confirmation = page.need((element) =>
      hasClass(element, "public-command-notices__confirmation"),
    );
    expect(renderedText(confirmation)).toContain(t("command-abandon-description"));
    const cancel = button(page, "dialog-cancel", confirmation);
    expect(cancel.attributes.autofocus).toBe(true);
    dispatch(cancel, "Click");
    expect(notice(page)).toBe(section);
    dispatch(abandon, "Click");
    confirmation = page.need((element) =>
      hasClass(element, "public-command-notices__confirmation"),
    );
    dispatch(button(page, "command-abandon", confirmation), "Click");
    expect(page.find((element) => hasClass(element, NOTICE))).toBeNull();
    expect(page.commands).toHaveLength(1);
    expect(findText(page, t("command-unknown-count", { count: 0 }))).not.toBeNull();
  } finally {
    page.dispose();
  }
});

test.each([ALICE, BOB])("经过实际登录页的往返按账号归属恢复命令（$username）", async (identity) => {
  const page = await mountCompetition();
  try {
    const first = await showUnknown(page);
    page.go("/login?return_to=%2F");
    await waitFor(() => page.find((element) => element.attributes.name === "login_key") !== null);
    expect(page.find((element) => hasClass(element, "public-auction-page"))).toBeNull();
    expect(
      page.find(
        (element) => element.tag === "form" && hasClass(element, "public-auction-bid-form"),
      ),
    ).toBeNull();
    expect(notice(page)).not.toBeNull();
    page.server.nextLogin = identity;
    const key = page.need((element) => element.attributes.name === "login_key");
    Object.assign(key, { value: "fixture-key" });
    dispatch(key, "Input");
    dispatch(
      page.need((element) => hasClass(element, "public-account-login-form")),
      "Submit",
    );
    await waitFor(
      () =>
        page.location.pathname === "/" &&
        page.find((element) => hasClass(element, "public-auction-page")) !== null,
    );
    if (identity === ALICE) {
      await waitFor(() => button(page, "data-retry", notice(page)).attributes.disabled !== true);
      const replay = await retry(page, first);
      replay.respond(bidReceipt());
    } else {
      expect(page.find((element) => hasClass(element, NOTICE))).toBeNull();
      await openBid(page);
      const newCommand = await confirm(page, bidForm(page));
      expect(newCommand.expectedUser).toBe(BOB.user_id);
      expect(newCommand.body).not.toBe(first.body);
      newCommand.respond(bidReceipt());
    }
  } finally {
    page.dispose();
  }
});

test("401 会暂停同身份重试，复核失败保留记录，入口限流与回放冲突继续保留", async () => {
  const page = await mountCompetition();
  try {
    const first = await showUnknown(page);
    page.go("/login");
    const section = notice(page);
    page.server.readSession = () => reject("storage_unavailable", 503);
    const authentication = await retry(page, first);
    authentication.respond(brokenBody(401));
    await waitFor(
      () =>
        page.find(
          (element) => renderedText(element) === t("command-retry-session-pending"),
          section,
        ) !== null,
    );
    const retryButton = button(page, "data-retry", section);
    const status = page.need((element) => hasClass(element, ENTRY_STATUS), section);
    const pause = page.need((element) => String(element.attributes.id).endsWith("-pause"), status);
    expect(retryButton.attributes.disabled).toBe(true);
    const pauseId = `public-command-notice-${LOT_ID}-pause`;
    expect(pause.getAttribute("id")).toBe(pauseId);
    expect(retryButton.getAttribute("aria-describedby")?.split(" ")).toContain(pauseId);
    expect(status.attributes.role).toBe("status");
    clickRetry(page);
    await settle();
    expect(page.commands).toHaveLength(2);

    for (const malformed of [
      ["ok", "", null],
      { code: "ok", message: "" },
      { code: "invalid", message: "", data: null },
    ]) {
      page.server.readSession = () => Response.json(malformed);
      requestPublicSessionCheck("authentication_failed");
      await settle();
      expect(notice(page)).toBe(section);
      expect(retryButton.attributes.disabled).toBe(true);
    }
    page.server.readSession = () => Response.json({ code: "ok", message: "", data: ALICE });
    requestPublicSessionCheck("authentication_failed");
    await waitFor(() => retryButton.attributes.disabled !== true);

    const malformedConflict = await retry(page, first);
    malformedConflict.respond(
      Response.json({ code: "session_changed", message: "" }, { status: 409 }),
    );
    await waitFor(() => retryButton.attributes.disabled !== true);
    expect(notice(page)).toBe(section);
    expect(renderedText(status)).toContain(t("error-invalid-response"));

    // 复核抢在重试发送前，仍保留原始未知结果及正文。
    clickRetry(page);
    page.server.readSession = () => reject("storage_unavailable", 503);
    requestPublicSessionCheck("authentication_failed");
    await settle();
    expect(page.commands).toHaveLength(3);
    expect(renderedText(status)).toContain(t("error-invalid-response"));
    page.server.readSession = () => success(ALICE);
    requestPublicSessionCheck("authentication_failed");
    await waitFor(() => retryButton.attributes.disabled !== true);

    const throttled = await retry(page, first);
    throttled.respond(reject("rate_limited", 429));
    await waitFor(() => retryButton.attributes.disabled !== true);
    expect(notice(page)).toBe(section);
    const conflict = await retry(page, first);
    const beforeReads = page.reads.length;
    conflict.respond(reject("command_conflict", 409));
    await waitFor(() => renderedText(status).includes(t("command-replay-conflict")));
    expect(notice(page)).toBe(section);
    expect(retryButton.attributes.disabled).toBe(false);
    await waitFor(() =>
      page.reads.slice(beforeReads).some((path) => path.startsWith("/api/v1/auctions/current?")),
    );
    const decision = await retry(page, first);
    decision.respond(reject("bid_too_low", 409));
    await waitFor(() => page.find((element) => hasClass(element, RECEIPT)) !== null);
    expect(page.find((element) => hasClass(element, NOTICE))).toBeNull();
    expect(renderedText(page.need((element) => hasClass(element, RECEIPT)))).toContain(
      t("command-retry-price-changed"),
    );
  } finally {
    page.dispose();
  }
});

test("首次明确拒绝收口，离开原弹窗后回显原因", async () => {
  const page = await mountCompetition();
  try {
    await openBid(page);
    const first = await confirm(page, bidForm(page));
    page.go("/login");
    first.respond(reject("command_conflict", 409));
    await waitFor(() => page.find((element) => hasClass(element, RECEIPT)) !== null);
    expect(renderedText(page.need((element) => hasClass(element, RECEIPT)))).toContain(
      t("command-conflict"),
    );
    expect(page.find((element) => hasClass(element, NOTICE))).toBeNull();
  } finally {
    page.dispose();
  }
});

test("确认交接后进入复核，首次请求明确反馈未发出", async () => {
  const page = await mountCompetition();
  try {
    await openBid(page);
    const form = bidForm(page);
    dispatch(form, "Submit");
    dispatch(form, "Submit");
    page.server.readSession = () => reject("storage_unavailable", 503);
    requestPublicSessionCheck("authentication_failed");
    await waitFor(() => findText(page, t("command-not-sent")) !== null);
    expect(page.commands).toHaveLength(0);
    expect(page.find((element) => hasClass(element, NOTICE))).toBeNull();
  } finally {
    page.dispose();
  }
});

test.each(["logout", "matching-conflict", "cookie-sync-conflict"] as const)(
  "已确认退出或预期用户冲突会撤销记录、回执和在途任务（%s）",
  async (source) => {
    const page = await mountCompetition();
    try {
      const first = await showUnknown(page);
      page.go("/login");
      const accepted = await retry(page, first);
      accepted.respond(bidReceipt());
      await waitFor(() => page.find((element) => hasClass(element, RECEIPT)) !== null);
      page.go("/");
      const unknown = await showUnknown(page, reject("command_not_sent", 503));
      page.go("/login");
      page.go("/");
      await openBid(page, "Clo02");
      const inFlight = await confirm(page, bidForm(page));
      page.go("/login");
      page.server.readSession = () => reject("storage_unavailable", 503);
      if (source === "logout") {
        dispatch(
          page.need((element) => hasClass(element, "public-account-session-trigger")),
          "Click",
        );
        dispatch(
          page.need((element) => element.tag === "button" && renderedText(element) === t("logout")),
          "Click",
        );
      } else if (source === "matching-conflict") {
        const conflict = await retry(page, unknown);
        conflict.respond(reject("session_changed", 409));
      } else {
        invalidateCookieSync();
        page.server.readSession = () => {
          page.server.readSession = () => reject("storage_unavailable", 503);
          return success(ALICE);
        };
        page.server.syncSession = () => reject("session_changed", 409);
        requestPublicSessionCheck("refresh");
      }
      await waitFor(() => inFlight.signal.aborted);
      expect(page.find((element) => hasClass(element, NOTICE))).toBeNull();
      expect(page.find((element) => hasClass(element, RECEIPT))).toBeNull();
    } finally {
      page.dispose();
    }
  },
);

test("A 的迟到 409 只复核 Cookie，B 已确认的宿主继续持有原命令", async () => {
  const page = await mountCompetition();
  try {
    await openBid(page);
    page.go("/login?return_to=%2F");
    await waitFor(() => page.find((element) => element.attributes.name === "login_key") !== null);
    page.server.nextLogin = BOB;
    const key = page.need((element) => element.attributes.name === "login_key");
    Object.assign(key, { value: "fixture-key" });
    dispatch(key, "Input");
    dispatch(
      page.need((element) => hasClass(element, "public-account-login-form")),
      "Submit",
    );
    await waitFor(() => page.location.pathname === "/");
    const first = await showUnknown(page);
    expect(first.expectedUser).toBe(BOB.user_id);
    page.go("/login");
    page.server.logout = () => reject("session_changed", 409);
    page.server.readSession = () => reject("storage_unavailable", 503);
    expect(await logoutPublic(ALICE.user_id, new AbortController().signal)).toEqual({
      ok: false,
      error: "session_changed",
    });
    await waitFor(() => button(page, "data-retry", notice(page)).attributes.disabled === true);
    expect(renderedText(notice(page))).toContain("Legendary Cotton Robe");
    expect(page.location.searchParams.get("auction")).not.toBe(AUCTION_ID);
  } finally {
    page.dispose();
  }
});
