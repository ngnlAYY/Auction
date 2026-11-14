//! Worker control Unix socket 的 peer 凭据过滤监听器。

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};

use crate::worker::diagnostics;
use std::collections::HashSet;
use std::time::Duration;

#[cfg(unix)]
pub(super) struct PeerCheckedUnixListener {
    pub(super) listener: tokio::net::UnixListener,
    pub(super) allowed_peer_uids: HashSet<u32>,
    pub(super) allowed_peer_gids: HashSet<u32>,
}

#[cfg(unix)]
impl axum::serve::Listener for PeerCheckedUnixListener {
    type Io = tokio::net::UnixStream;
    type Addr = tokio::net::unix::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.listener.accept().await {
                Ok((stream, address)) => match stream.peer_cred() {
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
                },
                Err(error) => {
                    diagnostics::error(
                        "control_listener",
                        "accept_connection",
                        error.diagnostic_kind(),
                        DiagnosticContext::default(),
                    );
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}
