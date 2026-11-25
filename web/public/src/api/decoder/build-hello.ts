// 实时 WebSocket 控制帧：建连首帧的服务端构建号。
// 与业务事件共用严格 JSON 边界，但形状独立：只接受 `{"type":"hello","build":"<构建号>"}`，
// 另允许且只允许一个可选字符串字段 `independent_build`（独立入口 Solid 的构建号）；
// 任何其他多余、缺失或非法值都返回 null，由事件解码路径按未知帧处理。

import { jsonKeysOnly, parseJson } from "@auction/web-shared/net/wire";
import { buildIdIsValid } from "@auction/web-shared/ui/bootstrap";
import { buildReportedToSolid } from "@auction/web-shared/ui/build-notice";

import { object, string } from "./index";

/**
 * 解析构建号首帧，返回 Solid 页面应比较的构建号：有 `independent_build` 时取它，
 * 否则取 `build`（与原行为一致）。返回非 null 时调用方只更新过期提示，不进入事件序列。
 */
export function decodeBuildHello(message: string): string | null {
  const parsed = parseJson(message);
  if (!parsed.ok) {
    return null;
  }
  const decoded = object(parsed.value);
  if (decoded === null) {
    return null;
  }
  if (!jsonKeysOnly(decoded, ["type", "build"], ["independent_build"])) {
    return null;
  }
  if (string(decoded.type) !== "hello") {
    return null;
  }
  const build = string(decoded.build);
  if (build === null || !buildIdIsValid(build)) {
    return null;
  }
  if (decoded.independent_build === undefined) {
    return build;
  }
  const independent = string(decoded.independent_build);
  return independent !== null && buildIdIsValid(independent)
    ? buildReportedToSolid(build, independent)
    : null;
}
