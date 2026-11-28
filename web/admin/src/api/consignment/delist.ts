// 运营下架与止售共用的私有说明校验；命令正文由 v2 隔离 API 发送。

import { DELIST_REASON } from "@auction/contracts/dto/admin/consignment";
import { is } from "valibot";

import { trimRustWhitespace } from "../session-time";

/** 采用下架说明自身的闭集判断输入，空白表单单独显示必填提示。 */
export function delistReasonError(reason: string): "required" | "invalid" | null {
  if (trimRustWhitespace(reason).length === 0) return "required";
  // 直接用说明 schema 校验，避免固定 UUID/版本样例借用请求 schema 引入无关失败。
  return is(DELIST_REASON, reason) ? null : "invalid";
}
