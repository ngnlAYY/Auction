//! systemctl 的同步、有界管道读取与直接子进程回收，不等待继承管道的其他进程。

use std::{
    io::{self, Read},
    os::fd::AsRawFd,
    process::{Child, Output},
    time::{Duration, Instant},
};

use anyhow::Context;

use super::{MAX_SYSTEMCTL_DIAGNOSTIC_BYTES, MAX_SYSTEMCTL_OUTPUT_BYTES};

const POLL_INTERVAL: Duration = Duration::from_millis(10);
const REAP_TIMEOUT: Duration = Duration::from_secs(5);

/// 两个非阻塞管道交替读取，避免双管道互锁；所有读取与退出确认共享同一截止时刻。
pub(super) fn capture_output(mut child: Child, deadline: Instant) -> anyhow::Result<Output> {
    let result = (|| {
        let mut stdout = child.stdout.take().context("systemctl stdout pipe is missing")?;
        let mut stderr = child.stderr.take().context("systemctl stderr pipe is missing")?;
        nonblocking(&stdout)?;
        nonblocking(&stderr)?;
        let mut output = Vec::new();
        let mut diagnostic = Vec::with_capacity(MAX_SYSTEMCTL_DIAGNOSTIC_BYTES);
        let mut stdout_closed = false;
        let mut stderr_closed = false;
        let mut status = None;
        loop {
            if Instant::now() >= deadline {
                anyhow::bail!("systemctl 执行超时；unit job 可能仍在进行，须重新核对状态");
            }
            let read_stdout = read_pipe(
                &mut stdout,
                &mut output,
                &mut stdout_closed,
                MAX_SYSTEMCTL_OUTPUT_BYTES,
                true,
            )?;
            let read_stderr = read_pipe(
                &mut stderr,
                &mut diagnostic,
                &mut stderr_closed,
                MAX_SYSTEMCTL_DIAGNOSTIC_BYTES,
                false,
            )?;
            if status.is_none() {
                status = child.try_wait().context("读取 systemctl 退出状态失败")?;
            }
            if stdout_closed
                && stderr_closed
                && let Some(status) = status
            {
                if Instant::now() >= deadline {
                    anyhow::bail!("systemctl 执行超时；unit job 可能仍在进行，须重新核对状态");
                }
                return Ok(Output { status, stdout: output, stderr: diagnostic });
            }
            if !read_stdout && !read_stderr {
                pause_until(deadline);
            }
        }
    })();
    // 读取失败或超时后先关闭两条管道，再终止并回收直接子进程；不留下阻塞读取线程。
    match result {
        Ok(output) => Ok(output),
        Err(error) => match terminate_and_reap(&mut child) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(error.context(format!("systemctl 清理失败：{cleanup:#}"))),
        },
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
    // SAFETY：管道在调用期间保持打开；fcntl 只读取或更新该描述符的状态标志。
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY：沿用同一打开管道的现有标志，只额外设置非阻塞读取。
    if unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// 每轮每管道至多读取一个块；持续输出也必须返回主循环检查截止时刻。
fn read_pipe(
    pipe: &mut impl Read,
    retained: &mut Vec<u8>,
    closed: &mut bool,
    limit: usize,
    reject_oversize: bool,
) -> anyhow::Result<bool> {
    if *closed {
        return Ok(false);
    }
    let mut buffer = [0_u8; 8192];
    match pipe.read(&mut buffer) {
        Ok(0) => {
            *closed = true;
            Ok(false)
        }
        Ok(count) => {
            let keep = count.min(limit.saturating_sub(retained.len()));
            if reject_oversize && keep != count {
                anyhow::bail!("systemctl 输出超过 64 KiB 上限");
            }
            retained.extend_from_slice(&buffer[..keep]);
            Ok(true)
        }
        Err(error)
            if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) =>
        {
            Ok(false)
        }
        Err(error) => Err(error).context("读取 systemctl 管道失败"),
    }
}

fn terminate_and_reap(child: &mut Child) -> anyhow::Result<()> {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return Ok(());
    }
    let killed = child.kill();
    let deadline = Instant::now() + REAP_TIMEOUT;
    loop {
        if child.try_wait().context("回收 systemctl 子进程失败")?.is_some() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            killed.context("强制终止 systemctl 失败")?;
            anyhow::bail!("强制终止 systemctl 后五秒内未确认直接子进程回收");
        }
        pause_until(deadline);
    }
}

fn pause_until(deadline: Instant) {
    std::thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
}
