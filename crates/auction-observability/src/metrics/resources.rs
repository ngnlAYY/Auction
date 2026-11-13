//! 当前进程的资源指标：从 `/proc/self` 读取 CPU、内存、文件描述符、线程数与启动时间。

use std::fs;

use prometheus::{
    IntCounter, IntGauge, Opts,
    core::{Collector, Desc},
    proto::MetricFamily,
};

use super::types::MetricsError;

/// 内核向用户态报告 CPU 时间与启动时间所用的每秒 tick 数。Linux 在受支持的架构上固定为 100，
/// Prometheus 官方 procfs 库同样按常量处理；不为一次 `sysconf` 引入 unsafe 调用。
const USER_HZ: u64 = 100;

/// Prometheus 约定的 `process_*` 指标；名称、类型与单位是既有监控面板和规则的契约。
/// 只读取当前进程，不带连接串、请求参数或业务主键。
pub(super) struct ProcessResourceCollector {
    descs: Vec<Desc>,
    cpu_seconds: IntCounter,
    open_fds: IntGauge,
    max_fds: IntGauge,
    virtual_memory: IntGauge,
    resident_memory: IntGauge,
    start_time: IntGauge,
    threads: IntGauge,
}

impl ProcessResourceCollector {
    pub(super) fn new() -> Result<Self, MetricsError> {
        let cpu_seconds = IntCounter::with_opts(Opts::new(
            "process_cpu_seconds_total",
            "Total user and system CPU time spent in seconds.",
        ))
        .map_err(MetricsError::Registry)?;
        let open_fds = gauge("process_open_fds", "Number of open file descriptors.")?;
        let max_fds = gauge("process_max_fds", "Maximum number of open file descriptors.")?;
        let virtual_memory =
            gauge("process_virtual_memory_bytes", "Virtual memory size in bytes.")?;
        let resident_memory =
            gauge("process_resident_memory_bytes", "Resident memory size in bytes.")?;
        let start_time = gauge(
            "process_start_time_seconds",
            "Start time of the process since unix epoch in seconds.",
        )?;
        let threads = gauge("process_threads", "Number of OS threads in the process.")?;

        // 启动时间在进程生命周期内不变，只在注册时读取一次。
        if let Some(stat) = read("/proc/self/stat").as_deref().and_then(parse_stat)
            && let Some(boot_time) = read("/proc/stat").as_deref().and_then(parse_boot_time)
        {
            start_time.set(saturating_i64(stat.start_ticks / USER_HZ + boot_time));
        }

        let descs = cpu_seconds
            .desc()
            .into_iter()
            .chain(open_fds.desc())
            .chain(max_fds.desc())
            .chain(virtual_memory.desc())
            .chain(resident_memory.desc())
            .chain(start_time.desc())
            .chain(threads.desc())
            .cloned()
            .collect();
        Ok(Self {
            descs,
            cpu_seconds,
            open_fds,
            max_fds,
            virtual_memory,
            resident_memory,
            start_time,
            threads,
        })
    }
}

impl Collector for ProcessResourceCollector {
    fn desc(&self) -> Vec<&Desc> {
        self.descs.iter().collect()
    }

    /// 任一来源读取失败时保留上一次的值；`/proc/self/stat` 不可读时不导出 CPU 计数。
    fn collect(&self) -> Vec<MetricFamily> {
        let mut families = Vec::with_capacity(self.descs.len());
        if let Some(stat) = read("/proc/self/stat").as_deref().and_then(parse_stat) {
            // 计数器只增不减：按整秒总量与已累计值的差推进。
            let total_seconds = (stat.user_ticks + stat.system_ticks) / USER_HZ;
            self.cpu_seconds.inc_by(total_seconds.saturating_sub(self.cpu_seconds.get()));
            self.virtual_memory.set(saturating_i64(stat.virtual_memory_bytes));
            self.threads.set(saturating_i64(stat.threads));
            families.extend(self.cpu_seconds.collect());
        }
        if let Some(count) = open_fd_count() {
            self.open_fds.set(saturating_i64(count));
        }
        if let Some(limit) = read("/proc/self/limits").as_deref().and_then(parse_max_open_files) {
            self.max_fds.set(saturating_i64(limit));
        }
        if let Some(kilobytes) =
            read("/proc/self/status").as_deref().and_then(parse_resident_kilobytes)
        {
            self.resident_memory.set(saturating_i64(kilobytes.saturating_mul(1024)));
        }
        families.extend(self.open_fds.collect());
        families.extend(self.max_fds.collect());
        families.extend(self.virtual_memory.collect());
        families.extend(self.resident_memory.collect());
        families.extend(self.start_time.collect());
        families.extend(self.threads.collect());
        families
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ProcessStat {
    user_ticks: u64,
    system_ticks: u64,
    threads: u64,
    start_ticks: u64,
    virtual_memory_bytes: u64,
}

fn gauge(name: &str, help: &str) -> Result<IntGauge, MetricsError> {
    IntGauge::with_opts(Opts::new(name, help)).map_err(MetricsError::Registry)
}

fn read(path: &str) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn saturating_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// `comm` 可以包含空格与括号，字段从最后一个 `)` 之后开始计数；其后首个是 proc(5) 的第 3 字段。
fn parse_stat(content: &str) -> Option<ProcessStat> {
    let (_, after_comm) = content.rsplit_once(')')?;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    let field = |number: usize| fields.get(number - 3)?.parse::<u64>().ok();
    Some(ProcessStat {
        user_ticks: field(14)?,
        system_ticks: field(15)?,
        threads: field(20)?,
        start_ticks: field(22)?,
        virtual_memory_bytes: field(23)?,
    })
}

fn parse_boot_time(content: &str) -> Option<u64> {
    content.lines().find_map(|line| line.strip_prefix("btime "))?.trim().parse().ok()
}

/// 软限制为 `unlimited` 时没有可导出的数值。
fn parse_max_open_files(content: &str) -> Option<u64> {
    content
        .lines()
        .find_map(|line| line.strip_prefix("Max open files"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// `status` 以 kB 报告常驻内存，不依赖页大小。
fn parse_resident_kilobytes(content: &str) -> Option<u64> {
    content
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Linux 6.2 起目录大小即描述符数量，无需打开目录；更早的内核逐项计数。
fn open_fd_count() -> Option<u64> {
    let reported = fs::metadata("/proc/self/fd").ok()?.len();
    if reported > 0 {
        return Some(reported);
    }
    u64::try_from(fs::read_dir("/proc/self/fd").ok()?.count()).ok()
}

#[cfg(test)]
mod tests {
    use prometheus::{Registry, core::Collector};

    use super::{
        ProcessResourceCollector, ProcessStat, parse_boot_time, parse_max_open_files,
        parse_resident_kilobytes, parse_stat,
    };

    #[test]
    fn stat_fields_are_counted_after_the_last_parenthesis_of_comm() {
        let content = "4242 (tokio) rt (x) S 1 4242 4242 0 -1 4194560 2817 0 0 0 731 219 0 0 20 0 \
                       9 0 880123 1234567168 5120 18446744073709551615 1 1 0 0 0 0 0 4096 17475 \
                       0 0 0 17 3 0 0 0 0 0 0 0 0 0 0 0 0 0\n";
        assert_eq!(
            parse_stat(content),
            Some(ProcessStat {
                user_ticks: 731,
                system_ticks: 219,
                threads: 9,
                start_ticks: 880_123,
                virtual_memory_bytes: 1_234_567_168,
            })
        );
        assert_eq!(parse_stat("4242 (short) S 1 2 3"), None);
        assert_eq!(parse_stat("no comm at all"), None);
    }

    #[test]
    fn procfs_text_sources_yield_boot_time_limits_and_resident_memory() {
        assert_eq!(
            parse_boot_time("cpu  1 2 3\nbtime 1763000000\nprocesses 9\n"),
            Some(1_763_000_000)
        );
        assert_eq!(parse_boot_time("cpu  1 2 3\n"), None);

        let limits = "Limit                     Soft Limit           Hard Limit           Units\n\
                      Max open files            65536                524288               files\n";
        assert_eq!(parse_max_open_files(limits), Some(65_536));
        assert_eq!(
            parse_max_open_files(
                "Max open files            unlimited            unlimited            files\n"
            ),
            None
        );

        assert_eq!(
            parse_resident_kilobytes("VmSize:\t  204800 kB\nVmRSS:\t   51200 kB\n"),
            Some(51_200)
        );
        assert_eq!(parse_resident_kilobytes("Name:\tkthreadd\n"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn collector_exports_the_seven_prometheus_process_families_for_this_process() {
        let registry = Registry::new();
        let collector = ProcessResourceCollector::new().unwrap();
        assert_eq!(collector.desc().len(), 7);
        registry.register(Box::new(collector)).unwrap();

        let families = registry.gather();
        let value = |name: &str| {
            let family = families
                .iter()
                .find(|family| family.name() == name)
                .unwrap_or_else(|| panic!("missing {name}"));
            let metric = &family.get_metric()[0];
            if name.ends_with("_total") {
                metric.get_counter().get_value()
            } else {
                metric.get_gauge().get_value()
            }
        };
        assert_eq!(families.len(), 7);
        assert!(value("process_cpu_seconds_total") >= 0.0);
        assert!(value("process_open_fds") >= 3.0);
        assert!(value("process_max_fds") >= value("process_open_fds"));
        assert!(value("process_virtual_memory_bytes") >= value("process_resident_memory_bytes"));
        assert!(value("process_resident_memory_bytes") > 0.0);
        assert!(value("process_threads") >= 1.0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let started = value("process_start_time_seconds");
        // 启动时间由开机时间加启动 tick 推出，必然落在最近一天内且不晚于当前时刻。
        assert!(started <= now + 1.0 && started > now - 86_400.0, "{started} vs {now}");
    }
}
