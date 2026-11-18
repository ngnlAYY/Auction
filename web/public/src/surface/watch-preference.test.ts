// 账号提醒偏好在真实 GET、PUT 边界上区分论坛私信资格失效与其他拒绝。

import { expect, test } from "bun:test";
import { t } from "@auction/web-shared/ui/i18n";
import {
  dispatch,
  mountCompetition,
  renderedText,
  settle,
  waitFor,
} from "./commands/competition-fixture.test";
import { ALICE, reject, success } from "./shell/session-fixture";

test.each([
  ["pm_channel_unavailable", 422, false, "outbid-pm-unavailable"],
  ["pm_channel_unavailable", 400, false, "outbid-save-error"],
  ["invalid_request", 422, false, "outbid-save-error"],
  ["pm_channel_unavailable", 422, true, "outbid-pm-unavailable"],
  ["pm_channel_unavailable", 400, true, "outbid-unknown"],
  ["invalid_request", 422, true, "outbid-unknown"],
] as const)(
  "保存拒绝 %s（HTTP %i，重放 %s）显示对应结果",
  async (code, status, replay, message) => {
    const page = await mountCompetition("main", true);
    const path = "/api/v1/me/lot-watch-preference";
    const methods: string[] = [];
    const sentBodies: unknown[] = [];
    let reads = 0;
    try {
      page.server.readPage = (url, init) => {
        if (url.pathname !== path) return null;
        const method = init?.method ?? "GET";
        methods.push(method);
        if (method === "PUT") {
          sentBodies.push(init?.body);
          if (replay && sentBodies.length === 1) {
            return reject("temporarily_unavailable", 503);
          }
          return reject(code, status);
        }
        reads += 1;
        return success({
          user_id: ALICE.user_id,
          channel: reads === 1 ? "pm" : "none",
          version: reads,
          pm_available: reads === 1,
        });
      };
      page.go("/me");
      const enabled = (label: string) =>
        page.find(
          (element) =>
            element.tag === "button" &&
            element.attributes.disabled !== true &&
            renderedText(element) === t(label),
        );
      await waitFor(() => enabled("outbid-save") !== null);
      const save = enabled("outbid-save");
      if (save === null) throw new Error("未挂载保存入口");
      const location = page.location.href;
      dispatch(save, "Click");
      const feedback = page.need((element) => element.getAttribute("id") === "outbid-feedback");
      const writes = ["GET", "PUT"];
      if (replay) {
        await waitFor(() => enabled("outbid-retry-command") !== null);
        const retry = enabled("outbid-retry-command");
        if (retry === null) throw new Error("未挂载原样重试入口");
        expect(renderedText(feedback)).toBe(t("outbid-unknown"));
        expect(enabled("outbid-refresh")).toBeNull();
        expect(methods).toEqual(writes);
        dispatch(retry, "Click");
        writes.push("PUT");
        await waitFor(() => methods.length === writes.length);
        expect(sentBodies[1]).toBe(sentBodies[0]);
      }
      await settle();
      expect(feedback.getAttribute("aria-live")).toBe("polite");
      expect(renderedText(feedback)).toBe(t(message));
      expect(methods).toEqual(writes);
      expect(sentBodies[0]).toEqual(expect.stringContaining('"channel":"pm","expected_version":1'));
      expect(page.location.href).toBe(location);
      if (message === "outbid-unknown") {
        expect(enabled("outbid-retry-command")).not.toBeNull();
        expect(enabled("outbid-refresh")).toBeNull();
        return;
      }

      expect(enabled("outbid-retry-command")).toBeNull();
      const refresh = enabled("outbid-refresh");
      if (refresh === null) throw new Error("未挂载重读入口");
      dispatch(refresh, "Click");
      const pm = () => page.need((element) => element.getAttribute("id") === "outbid-channel-pm");
      await waitFor(() => reads === 2 && enabled("outbid-save") !== null);
      expect(pm().attributes.disabled).toBe(true);
      expect(renderedText(feedback)).toBe("");
      expect(methods).toEqual([...writes, "GET"]);
      expect(page.location.href).toBe(location);
    } finally {
      page.dispose();
    }
  },
);
