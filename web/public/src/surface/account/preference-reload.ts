// 两项账号偏好消费同账号重登信号，等未知写入结束后合并积压的读取。

import { createEffect, untrack, useContext } from "solid-js";
import { PreferenceReloadRevisionContext } from "../shell/contexts";

/** 同一宿主只保留最近的待读取修订，显式重读和写入后的读取也可消费该修订。 */
export function usePreferenceReload(
  ready: () => boolean,
  blocked: () => boolean,
  load: (preserveDraft: boolean) => void,
): () => void {
  const revision = useContext(PreferenceReloadRevisionContext);
  let readRevision: number | null = null;
  createEffect(
    () => ({ revision: revision(), ready: ready() && !blocked() }),
    (next) => {
      if (!next.ready || readRevision === next.revision) {
        return;
      }
      const preserveDraft = readRevision !== null;
      readRevision = next.revision;
      load(preserveDraft);
    },
  );
  return () => {
    readRevision = untrack(revision);
  };
}
