// 会话资源携带外壳权威读取时确认的主修订，供私有读取和写命令各自核对。

import { publicSessionPrimaryRevision } from "../../api/session/revision";
import type { PublicSessionResource } from "..";

/** 对象表示已确认，无共享修订键时其 revision 仍可为 null。 */
export interface SessionConfirmation {
  readonly revision: string | null;
}

/** 私有操作使用同一会话资源上的确认；无确认的资源不能授权发包。 */
export type PublicSessionAuthority = PublicSessionResource & {
  readonly confirmation?: () => SessionConfirmation | null;
};

/** 在点击和发包边界同步读共享修订，涵盖尚未处理 storage 事件的窗口。 */
export function currentSessionConfirmation(
  session: PublicSessionAuthority,
): SessionConfirmation | null {
  const confirmation = session.confirmation?.() ?? null;
  return confirmation?.revision === publicSessionPrimaryRevision() ? confirmation : null;
}
