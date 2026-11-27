// 真实目录行在场次读取失败、路由切换与账号更换后的代理快照生命周期。

import { expect, test } from "bun:test";
import { t } from "@auction/web-shared/ui/i18n";
import { requestPublicSessionCheck } from "../../api/session";
import { invalidateAll } from "../../api/swr-cache";
import {
  AUTHORIZATION_ID,
  confirm,
  dispatch,
  hasClass,
  LOT_ID,
  mountCompetition,
  openBid,
  ownedProxy,
  renderedText,
  settle,
  waitFor,
} from "../commands/competition-fixture.test";
import type { DomElement } from "../shell/navigation-fixture.test";
import { ALICE, BOB, reject, success } from "../shell/session-fixture";

const reasonId = `public-auction-action-reason-${LOT_ID}`;
const bid = (element: DomElement) =>
  element.tag === "button" &&
  element.getAttribute("aria-label") === t("catalog-bid-lot", { code: "Clo01" });

test.each(["lower", "create"] as const)(
  "授权 GET 挂起时不发代理 %s POST，成功后可以提交",
  async (kind) => {
    const page = await mountCompetition();
    const retryRead = Promise.withResolvers<Response>();
    const countProxy = () =>
      page.reads.filter((read) => read === `/api/v1/lots/${LOT_ID}/proxy-authorizations/mine`)
        .length;
    const proxyTab = (element: DomElement) =>
      element.tag === "button" && renderedText(element) === t("bid-tab-proxy");
    const retry = (element: DomElement) => hasClass(element, "ui-async-state__retry");
    const submit = (element: DomElement) =>
      element.tag === "button" &&
      renderedText(element) === t(kind === "lower" ? "proxy-lower-submit" : "proxy-create-submit");
    const form = () =>
      page.need(
        (element) => element.tag === "form",
        page.need((element) => hasClass(element, "public-auction-bid-form")),
      );
    const lower = kind === "lower";
    try {
      page.server.activeProxy = lower;
      page.server.waitingProxy = lower;
      await openBid(page);
      dispatch(page.need(proxyTab), "Click");
      await waitFor(() => page.find(submit)?.attributes.disabled === false);
      expect(countProxy()).toBe(1);
      dispatch(
        page.need((element) => hasClass(element, "ui-dialog__close")),
        "Click",
      );
      await settle();

      page.server.readProxy = () => reject("internal_error", 500);
      dispatch(
        page.need(
          (element) =>
            element.getAttribute("aria-label") === t("catalog-view-lot", { code: "Clo01" }),
        ),
        "Click",
      );
      await waitFor(() => countProxy() === 3);
      await settle();
      dispatch(
        page.need((element) => hasClass(element, "public-catalog-detail__bid")),
        "Click",
      );
      if (!lower) dispatch(page.need(proxyTab), "Click");
      expect(page.need(submit).attributes.disabled).toBe(true);

      page.server.readProxy = () => retryRead.promise;
      dispatch(page.need(retry), "Click");
      await waitFor(() => countProxy() === 4);
      dispatch(form(), "Submit");
      dispatch(form(), "Submit");
      await settle();
      expect(page.commands).toHaveLength(0);
      expect(page.need(submit).attributes.disabled).toBe(true);

      retryRead.resolve(success({ authorization: lower ? ownedProxy(true) : null }));
      await waitFor(() => page.find(submit)?.attributes.disabled === false);
      const sent = await confirm(page, form());
      expect(page.commands).toHaveLength(1);
      expect(sent.path).toBe(
        `/api/v1/lots/${LOT_ID}/proxy-authorizations${lower ? `/${AUTHORIZATION_ID}/lower` : ""}`,
      );
      sent.respond(reject("command_conflict", 409));
    } finally {
      retryRead.resolve(reject("internal_error", 500));
      page.dispose();
    }
  },
);

test("同一拍品授权重读两次失败仍限制普通出价，确认无授权后才恢复", async () => {
  const page = await mountCompetition();
  const countProxy = () =>
    page.reads.filter((read) => read.endsWith("/proxy-authorizations/mine")).length;
  const open = () =>
    dispatch(
      page.need(
        (element) =>
          element.getAttribute("aria-label") === t("catalog-view-lot", { code: "Clo01" }),
      ),
      "Click",
    );
  const close = () =>
    dispatch(
      page.need((element) => hasClass(element, "public-catalog-detail__close")),
      "Click",
    );
  const blocked = () => page.find(bid)?.attributes.disabled === true;
  try {
    page.server.activeProxy = true;
    page.server.waitingProxy = true;
    await waitFor(() => page.find(bid)?.attributes.disabled === false);
    open();
    await waitFor(blocked);
    close();

    page.server.readProxy = () => reject("internal_error", 500);
    open();
    await waitFor(() => countProxy() === 3);
    await settle();
    expect(blocked()).toBe(true);
    dispatch(
      page.need((element) => hasClass(element, "public-catalog-detail__bid")),
      "Click",
    );
    expect(
      page.find(
        (element) =>
          element.getAttribute("role") === "alert" &&
          renderedText(element).includes(t("proxy-load-error")),
      ),
    ).not.toBeNull();
    expect(
      page.need(
        (element) => element.tag === "button" && renderedText(element) === t("proxy-lower-submit"),
      ).attributes.disabled,
    ).toBe(true);
    dispatch(
      page.need((element) => hasClass(element, "ui-dialog__close")),
      "Click",
    );
    close();
    await settle();
    expect(blocked()).toBe(true);
    // 已有生效代理时行内不渲染说明节点。
    expect(page.find((element) => element.attributes.id === reasonId)).toBeNull();

    page.server.readProxy = () => null;
    page.server.activeProxy = false;
    open();
    await waitFor(() => page.find(bid)?.attributes.disabled === false);
    expect(countProxy()).toBe(4);
    expect(blocked()).toBe(false);
  } finally {
    page.dispose();
  }
});

test("等待反击的代理在同场恢复后仍拦普通出价，换号后销毁原快照", async () => {
  const page = await mountCompetition();
  const countProxy = () =>
    page.reads.filter((read) => read.endsWith("/proxy-authorizations/mine")).length;
  const blocked = () => page.find(bid)?.attributes.disabled === true;
  try {
    page.server.activeProxy = true;
    page.server.waitingProxy = true;
    await waitFor(() => page.find(bid)?.attributes.disabled === false);
    dispatch(
      page.need(
        (element) =>
          element.getAttribute("aria-label") === t("catalog-view-lot", { code: "Clo01" }),
      ),
      "Click",
    );
    await waitFor(blocked);
    dispatch(
      page.need((element) => hasClass(element, "public-catalog-detail__close")),
      "Click",
    );
    expect(blocked()).toBe(true);
    expect(countProxy()).toBe(1);

    page.server.readCurrent = () => reject("internal_error", 500);
    // 生产外壳复查会话后刷新公开读缓存，使当前场次真的走到 500 分支。
    invalidateAll();
    requestPublicSessionCheck("cookie_changed");
    const retry = (element: DomElement) => hasClass(element, "ui-async-state__retry");
    await waitFor(() => page.find(retry) !== null && page.find(bid) === null);
    page.server.readCurrent = () => null;
    dispatch(page.need(retry), "Click");
    await waitFor(() => page.find(bid) !== null);
    expect(blocked()).toBe(true);
    // 按钮不引用不存在的说明，页面上也没有这句话。
    expect(page.need(bid).getAttribute("aria-describedby") ?? "").not.toContain(reasonId);
    expect(page.find((element) => element.attributes.id === reasonId)).toBeNull();
    expect(page.find((element) => renderedText(element) === t("proxy-active-exists"))).toBeNull();
    expect(countProxy()).toBe(1);

    page.go("/rules");
    await settle();
    page.go("/");
    await waitFor(blocked);
    expect(countProxy()).toBe(1);
    for (const user of [BOB, ALICE]) {
      page.server.identity = user;
      requestPublicSessionCheck("refresh");
      await waitFor(() => page.find((element) => renderedText(element) === user.username) !== null);
      expect(blocked()).toBe(false);
      expect(page.find((element) => element.attributes.id === reasonId)).toBeNull();
    }
    expect(countProxy()).toBe(1);
  } finally {
    page.dispose();
  }
});
