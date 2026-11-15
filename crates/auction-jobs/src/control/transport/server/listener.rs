//! Worker control Unix socket 的 peer 凭据过滤监听器；socket 文件安全由装配根绑定的内层
//! listener 负责，这里只决定哪些已接受的连接可以进入控制路由。

use std::collections::HashSet;

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use axum::serve::Listener;

use crate::worker::diagnostics;

/// 只交付 peer UID 或 GID 在显式 allowlist 内的连接；其余连接在读取任何请求字节前关闭。
/// accept 错误的记录与退避由内层 listener 按 axum `Listener` 合同处理。
pub(super) struct PeerCheckedUnixListener<L> {
    pub(super) listener: L,
    pub(super) allowed_peer_uids: HashSet<u32>,
    pub(super) allowed_peer_gids: HashSet<u32>,
}

impl<L> Listener for PeerCheckedUnixListener<L>
where
    L: Listener<Io = tokio::net::UnixStream>,
{
    type Io = tokio::net::UnixStream;
    type Addr = L::Addr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (stream, address) = self.listener.accept().await;
            match stream.peer_cred() {
                Ok(credential)
                    if self.allowed_peer_uids.contains(&credential.uid())
                        || self.allowed_peer_gids.contains(&credential.gid()) =>
                {
                    return (stream, address);
                }
                Ok(credential) => {
                    tracing::warn!(
                        peer_uid = credential.uid(),
                        peer_gid = credential.gid(),
                        peer_pid = ?credential.pid(),
                        "Worker control 拒绝不在显式 UID/GID allowlist 内的 peer"
                    );
                }
                Err(error) => {
                    diagnostics::warn(
                        "control_listener",
                        "read_peer_credentials",
                        error.diagnostic_kind(),
                        DiagnosticContext::default(),
                    );
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fs;
    use std::os::unix::fs::DirBuilderExt;
    use std::time::Duration;

    use axum::serve::Listener;
    use tokio::net::{UnixListener, UnixStream};
    use uuid::Uuid;

    use super::PeerCheckedUnixListener;

    /// 服务端在交付前关闭被拒连接：客户端最终读到 EOF。
    async fn read_eof(stream: &UnixStream) -> bool {
        let mut buffer = [0_u8; 1];
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::timeout_at(deadline, stream.readable()).await.is_ok() {
            match stream.try_read(&mut buffer) {
                Ok(read) => return read == 0,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("读取被拒连接失败：{error}"),
            }
        }
        false
    }

    #[tokio::test]
    async fn only_allowlisted_peers_reach_control_routes() {
        let directory =
            std::env::temp_dir().join(format!("auction-control-peer-{}", Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&directory).unwrap();
        let path = directory.join("control.sock");
        let inner = UnixListener::bind(&path).unwrap();
        // 连接本进程监听的 socket 时，客户端看到的对端凭据就是本进程的有效 UID/GID。
        let rejected = UnixStream::connect(&path).await.unwrap();
        let own = rejected.peer_cred().unwrap();
        let mut listener = PeerCheckedUnixListener {
            listener: inner,
            allowed_peer_uids: HashSet::from([own.uid().wrapping_add(1)]),
            allowed_peer_gids: HashSet::new(),
        };
        assert!(tokio::time::timeout(Duration::from_millis(300), listener.accept()).await.is_err());
        assert!(read_eof(&rejected).await);

        listener.allowed_peer_gids.insert(own.gid());
        let _by_gid = UnixStream::connect(&path).await.unwrap();
        let (accepted, _) =
            tokio::time::timeout(Duration::from_secs(5), listener.accept()).await.unwrap();
        assert_eq!(accepted.peer_cred().unwrap().gid(), own.gid());

        listener.allowed_peer_gids.clear();
        listener.allowed_peer_uids.insert(own.uid());
        let _by_uid = UnixStream::connect(&path).await.unwrap();
        let (accepted, _) =
            tokio::time::timeout(Duration::from_secs(5), listener.accept()).await.unwrap();
        assert_eq!(accepted.peer_cred().unwrap().uid(), own.uid());
        drop(listener);
        fs::remove_dir_all(directory).unwrap();
    }
}
