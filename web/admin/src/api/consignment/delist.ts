// 运营下架与止售共用的私有说明校验；命令正文由 v2 隔离 API 发送。

import { AdminDelistListingSchema } from "@auction/contracts/dto/admin/consignment";
import { is } from "valibot";

import { trimRustWhitespace } from "../session-time";

/** 采用下架请求契约判断输入，空白表单单独显示必填提示。 */
export function delistReasonError(reason: string): "required" | "invalid" | null {
  if (trimRustWhitespace(reason).length === 0) return "required";
  // 请求 schema 同时校验 UUID 与版本；这两个固定样例只隔离待验证的原因。
  return is(AdminDelistListingSchema, {
    command_id: "00000000-0000-4000-8000-000000000001",
    expected_version: 1,
    reason,
  })
    ? null
    : "invalid";
}
