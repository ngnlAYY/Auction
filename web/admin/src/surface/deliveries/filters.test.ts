// 用实际表单提交检查非法时间与未编辑的精确筛选边界。

import { expect, test } from "bun:test";
import { createComponent } from "solid-js";
import {
  DELIVERY_RENDER,
  settle,
  withDeliveryDom,
  withTestTimezone,
} from "./delivery-dom-fixture.test";
import type { DeliveryFiltersDraft } from "./filters";

test("invalid_delivery_time_blocks_submit_and_unedited_prefill_keeps_precision", async () => {
  const { DeliveryFilters } = await DELIVERY_RENDER.filters();
  await withTestTimezone("UTC", () =>
    withDeliveryDom("https://auction.test/deliveries", async (dom, mount) => {
      const applied: DeliveryFiltersDraft[] = [];
      const from = "2026-11-18T09:05:37.500123456+08:00";
      const to = "2026-11-19T01:05:00Z";
      mount(() =>
        createComponent(DeliveryFilters, {
          status: "",
          recipient: "",
          reference: "",
          template: "",
          category: "",
          world: "",
          createdFrom: from,
          createdTo: to,
          onStatusChange: () => {},
          onFiltersChange: (filters) => applied.push(filters),
        }),
      );
      await settle();
      const form = dom.need((node) => node.tag === "form");
      const toInput = dom.byId("delivery-created-to");
      expect(toInput.value).toBe("2026-11-19T01:05");
      toInput.input("2026-11-31T01:05");
      form.submit();
      expect(applied).toEqual([]);
      expect(toInput.getAttribute("aria-invalid")).toBe("true");
      expect(toInput.getAttribute("aria-describedby")).toBe("delivery-created-to-error");
      toInput.input("2026-11-20T01:05");
      form.submit();
      expect(applied).toHaveLength(1);
      expect(applied[0]).toMatchObject({ createdFrom: from, createdTo: "2026-11-20T01:05:00Z" });
    }),
  );
});
