// Public 外壳 readiness HTTP 客户端。
// 外壳只在启动门禁与页面恢复可见时读取 /health/ready；HTTP 未就绪和网络/解码失败均映射为
// unavailable，这里不泄露后端错误正文，读取生命周期仍由页面 owner 管理。

import { apiResponseSchema, HealthReadyDataSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { transportErrorCode } from "@auction/web-shared/net/transport";
import { err, ok, type Result } from "@auction/web-shared/result";
import { reportBrowserError } from "@auction/web-shared/ui/bootstrap";
import { sendRequest } from "./clock";

/** 读取 Public 进程 readiness，并保留旧客户端的 JSON Accept 头与 10 秒请求上限。 */
export async function fetchPublicReadiness(signal: AbortSignal): Promise<Result<boolean, string>> {
  const sent = await sendRequest(
    () =>
      ok({
        url: "/health/ready",
        init: { method: "GET", headers: { Accept: "application/json" } },
      }),
    signal,
  );
  if (!sent.ok) {
    return err(transportErrorCode(sent.error));
  }
  if (signal.aborted) {
    return err("network_error");
  }
  const response = sent.value;
  const responseOk = response.status >= 200 && response.status < 300;
  // 重复 JSON 键在 JSON.parse 后已被最后一个值覆盖；这是已登记的已知差异，后端不输出重复键。
  // 源实现直接反序列化，失败返回解析器错误文本；页面只判断成功与否，这里统一为 invalid_response。
  const payload = response.json();
  if (!payload.ok) {
    reportBrowserError("dto_decode");
    return err("invalid_response");
  }
  const decoded = decode(apiResponseSchema(HealthReadyDataSchema), payload.value);
  if (!decoded.ok) {
    reportBrowserError("dto_decode");
    return err("invalid_response");
  }
  return ok(responseOk && decoded.value.data.ready);
}
