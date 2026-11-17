// Public 站点文档（规则、关于）HTTP DTO 与请求。两种文档的响应形状相同，只有路径不同。

import type { PublicRulesResponse } from "@auction/contracts/dto/public/account";
import { transportErrorCode } from "@auction/web-shared/net/transport";
import { err, type Result, unreachable } from "@auction/web-shared/result";
import { sendGet } from "./clock";
import { decodeErrorCode } from "./decoder";
import { decodeRules } from "./decoder/catalog";

/** 规则或关于页面的公开文档投影。 */
export type RulesDocument = PublicRulesResponse;

/** 公开端可阅读的站点文档种类。 */
export type SiteDocument = "rules" | "about";

function siteDocumentApiPath(kind: SiteDocument): string {
  switch (kind) {
    case "rules":
      return "/api/v1/rules";
    case "about":
      return "/api/v1/about";
    default:
      return unreachable(kind);
  }
}

/** “关于”尚未发布时服务端返回的确定错误码；页面把它展示为空状态而不是故障。 */
export const DOCUMENT_UNPUBLISHED = "about_unpublished";

/** 按种类与语言读取站点文档；“关于”未发布时返回稳定错误码。 */
export async function fetchSiteDocument(
  kind: SiteDocument,
  locale: string,
  signal: AbortSignal,
): Promise<Result<RulesDocument, string>> {
  const query = new URLSearchParams();
  query.append("lang", locale);
  const sent = await sendGet(`${siteDocumentApiPath(kind)}?${query.toString()}`, signal);
  if (!sent.ok) {
    return err(transportErrorCode(sent.error));
  }
  if (signal.aborted) {
    return err("network_error");
  }
  const response = sent.value;
  if (!responseOk(response)) {
    return err(decodeErrorCode(response));
  }
  return decodeRules(response);
}

function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status < 300;
}
