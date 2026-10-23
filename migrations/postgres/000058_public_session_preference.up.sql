-- Owner: auction-application：sessions。
-- 默认只改变后续登录；保留全部既有会话的截止与撤销事实，绝不回填为永久。
ALTER TABLE public.users
    ADD COLUMN public_session_duration_seconds bigint
        CHECK (public_session_duration_seconds IS NULL
            OR (public_session_duration_seconds BETWEEN 60 AND 31536000
                AND public_session_duration_seconds % 60 = 0)),
    ADD COLUMN public_session_preference_version bigint NOT NULL DEFAULT 1
        CHECK (public_session_preference_version > 0);

COMMENT ON COLUMN public.users.public_session_duration_seconds IS
    'Public 后续登录的固定时长秒数；NULL 为永久，有限值为 1 分钟至 365 天的整分钟数';
COMMENT ON COLUMN public.users.public_session_preference_version IS
    'Public 登录时长偏好的独立 CAS 版本；不改变用户认证 state_version';

ALTER TABLE public.public_sessions
    ALTER COLUMN expires_at DROP NOT NULL,
    ALTER COLUMN absolute_expires_at DROP NOT NULL,
    ADD CONSTRAINT public_sessions_deadline_pair_check CHECK (
        (expires_at IS NULL AND absolute_expires_at IS NULL)
        OR (
            expires_at IS NOT NULL AND absolute_expires_at IS NOT NULL
            AND isfinite(expires_at) AND isfinite(absolute_expires_at)
            AND expires_at > created_at AND absolute_expires_at >= expires_at
        )
    );

COMMENT ON COLUMN public.public_sessions.expires_at IS
    'Public 固定截止；与 absolute_expires_at 同为 NULL 表示永久，普通活动不续期';
COMMENT ON COLUMN public.public_sessions.absolute_expires_at IS
    '旧会话保留原绝对截止；新有限登录及当前会话偏好保存后与 expires_at 相等';
