// 实时 WebSocket 控制帧：建连首帧的服务端构建号。
// 与业务事件共用严格 JSON 边界，但形状独立：只接受 `{"type":"hello","build":"<构建号>"}`，
// 任何多余、缺失或非法值都返回 null，由事件解码路径按未知帧处理。

import { jsonKeysOnly, parseJson } from "@auction/web-shared/net/wire";
import { buildIdIsValid } from "@auction/web-shared/ui/bootstrap";

import { object, string } from "./index";

/** 解析构建号首帧；返回非 null 时调用方只更新过期提示，不进入事件序列。 */
export function decodeBuildHello(message: string): string | null {
  const parsed = parseJson(message);
  if (!parsed.ok) {
    return null;
  }
  const decoded = object(parsed.value);
  if (decoded === null) {
    return null;
  }
  if (!jsonKeysOnly(decoded, ["type", "build"], [])) {
    return null;
  }
  if (string(decoded.type) !== "hello") {
    return null;
  }
  const build = string(decoded.build);
  return build !== null && buildIdIsValid(build) ? build : null;
}
