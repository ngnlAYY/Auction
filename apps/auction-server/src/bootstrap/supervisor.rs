//! 三角色进程树所有权、顺序启动与有界反序回收。

use super::{configuration::RolePlan, probe};
use anyhow::{Context, bail};
use auction_process::TerminationSignals;
use std::{fs::File, os::fd::AsRawFd, process::Stdio, time::Duration};
use tokio::{
    process::{Child, Command},
    time::{Instant, sleep, timeout_at},
};

struct RoleChild {
    name: &'static str,
    process: Child,
}

pub(super) async fn run(
    plans: Vec<RolePlan>,
    ownership: File,
    mut signals: TerminationSignals,
    startup: Duration,
) -> anyhow::Result<()> {
    let mut children = Vec::new();
    let result = tokio::select! {
        biased;
        signal = signals.recv() => signal.map(|_| ()).map_err(anyhow::Error::from),
        result = start_and_watch(&plans, &ownership, &mut children, startup) => result,
    };
    let stopped = stop(&mut children, &mut signals).await;
    // 所有角色都完成回收尝试后才释放父进程的锁；未退出子进程仍持有继承的 owner.lock。
    drop(ownership);
    match (result, stopped) {
        (Err(error), Err(stop_error)) => {
            Err(error.context(format!("角色关闭也失败：{stop_error:#}")))
        }
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

async fn start_and_watch(
    plans: &[RolePlan],
    ownership: &File,
    children: &mut Vec<RoleChild>,
    startup: Duration,
) -> anyhow::Result<()> {
    let executable = std::env::current_exe()?;
    for plan in plans {
        ensure_running(children)?;
        let mut command = Command::new(&executable);
        command
            .arg("--role")
            .arg(plan.role.name())
            .arg("--config-file")
            .arg(&plan.path)
            .env_remove("AUCTION_CONFIG_FILE")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        let descriptor = ownership.as_raw_fd();
        // SAFETY：pre_exec 只调用 async-signal-safe 的 libc 操作，不分配或访问锁。
        // 子进程独立进程组，终端信号只由 supervisor 转发；父进程意外死亡则内核发送 SIGTERM。
        unsafe {
            let parent = libc::getpid();
            command.pre_exec(move || {
                if libc::setpgid(0, 0) != 0
                    || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0
                    || libc::fcntl(descriptor, libc::F_SETFD, 0) == -1
                {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
        children.push(RoleChild {
            name: plan.role.name(),
            process: command.spawn().context("spawn service role")?,
        });
        let deadline = Instant::now() + startup;
        loop {
            ensure_running(children)?;
            if timeout_at(deadline, probe::ready(plan)).await.unwrap_or(false)
                && Instant::now() < deadline
            {
                ensure_running(children)?;
                eprintln!("role={} ready", plan.role.name());
                break;
            }
            if Instant::now() >= deadline {
                bail!("role={} readiness timeout", plan.role.name());
            }
            sleep(Duration::from_millis(200)).await;
        }
    }
    eprintln!("auction-server ready: worker, admin, public");
    loop {
        ensure_running(children)?;
        sleep(Duration::from_millis(200)).await;
    }
}

fn ensure_running(children: &mut [RoleChild]) -> anyhow::Result<()> {
    for child in children {
        if let Some(status) = child.process.try_wait()? {
            bail!("role={} exited unexpectedly: {}", child.name, status);
        }
    }
    Ok(())
}

async fn stop(children: &mut [RoleChild], signals: &mut TerminationSignals) -> anyhow::Result<()> {
    // 每个角色至多 10 秒 drain + 30 秒 tracing；三角色顺序关闭的总预算固定为 135 秒。
    let deadline = Instant::now() + Duration::from_secs(135);
    let mut failed_status = None;
    let graceful = async {
        for child in children.iter_mut().rev() {
            let status = if let Some(status) = child.process.try_wait()? {
                status
            } else {
                if let Some(pid) = child.process.id() {
                    // SAFETY：只向仍由 Child 持有且未 wait 回收的 PID 发送终止信号。
                    if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                child.process.wait().await?
            };
            // 一项 drain 失败仍须回收其余角色；全部 wait 后才把失败传播给 supervisor。
            if !status.success() && failed_status.is_none() {
                failed_status = Some((child.name, status));
            }
        }
        Ok::<_, std::io::Error>(())
    };
    let complete = tokio::select! {
        biased;
        _ = signals.recv() => false,
        result = timeout_at(deadline, graceful) => {
            matches!(result, Ok(Ok(()))) && Instant::now() < deadline
        },
    };
    if !complete {
        for child in children.iter_mut().rev() {
            let _ = child.process.start_kill();
        }
        let mut reap_error = None;
        for child in children.iter_mut().rev() {
            let result = tokio::time::timeout(Duration::from_secs(5), child.process.wait())
                .await
                .context(
                    "forced role reap timeout; inherited ownership lock remains until process exit",
                )
                .and_then(|result| result.context("reap forced role"));
            // 单个 wait 失败也必须继续等待其他角色，不能把剩余 Child 留给运行时兜底回收。
            if let Err(error) = result
                && reap_error.is_none()
            {
                reap_error = Some(error);
            }
        }
        if let Some(error) = reap_error {
            return Err(error);
        }
        bail!("server shutdown forced or exceeded its drain budget");
    }
    if let Some((role, status)) = failed_status {
        bail!("role={role} exited unsuccessfully during shutdown: {status}");
    }
    Ok(())
}
