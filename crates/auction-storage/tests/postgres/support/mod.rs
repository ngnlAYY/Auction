//! PostgreSQL 18 集成测试夹具：临时集群、已迁移模板库与逐测试克隆库。
//!
//! 设置 `AUCTION_TEST_DATABASE_URL` 时复用该 URL 所在服务器的维护连接（只建库/删库，
//! 不改写 URL 指向的库）；否则用本机 PostgreSQL 18 二进制启动一次性集群。二进制目录
//! 依次取 `AUCTION_TEST_PG_BIN`、`PATH`、`/usr/lib/postgresql/18/bin`。
//!
//! 夹具先在新建的空库里执行全部内嵌迁移得到模板库，再由每个测试 `CREATE DATABASE ...
//! TEMPLATE` 克隆独立库。测试库在 `TestDatabase` 析构时 `DROP DATABASE ... WITH (FORCE)`；
//! 临时集群在最后一个测试释放共享句柄后停止服务并删除数据目录。并发测试各自持有一个
//! 强引用，因此集群不会被先完成的测试提前回收。
//!
//! `TestDatabase::store` 默认以一次性登录角色连接克隆库。该角色只是固定运行账号
//! `auction_runtime` 的成员、继承其授权，没有 superuser/CREATEDB/CREATEROLE，也不拥有任何对象，
//! 因此能证明被测事务在生产运行账号的最小权限下可以提交。随机密码只存在于进程内存。迁移与
//! DDL 夹具改用 `TestDatabase::maintenance_store`。
//!
//! 该模块只在 `test-db` feature 下编译，`just verify` 只做静态编译，不连接数据库。

use std::{
    env,
    future::Future,
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, LazyLock, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use auction_storage::{
    DatabaseConnectionProfile, DatabaseStore, postgres_expected_migration_version,
};
use sqlx::{AssertSqlSafe, Connection, PgConnection};
use tokio::sync::Mutex;
use url::Url;

/// 共享集群句柄；弱引用保证最后一个测试库释放后才触发集群清理。
static CLUSTER: LazyLock<Mutex<Weak<Cluster>>> = LazyLock::new(|| Mutex::new(Weak::new()));
static UNIQUE: AtomicU64 = AtomicU64::new(0);

/// 从模板库克隆出的独立测试数据库；析构时删除自身并可能触发集群清理。
pub struct TestDatabase {
    cluster: Arc<Cluster>,
    name: String,
}

impl TestDatabase {
    /// 克隆已执行全部迁移的模板库。
    pub async fn create() -> Result<Self, String> {
        Self::create_inner(false).await
    }

    /// 从 `template0` 新建空库，供测试自行执行迁移。
    pub async fn create_empty() -> Result<Self, String> {
        Self::create_inner(true).await
    }

    async fn create_inner(empty: bool) -> Result<Self, String> {
        let cluster = shared_cluster().await?;
        let name = unique_name("auction_test_db_");
        let source = if empty { "template0" } else { cluster.template.as_str() };
        // 标识符只由本模块生成的 ASCII 名称构成，仍统一加引号避免大小写与保留字歧义。
        let statement =
            format!("CREATE DATABASE {} TEMPLATE {}", quote_ident(&name), quote_ident(source));
        run_sql(&cluster.admin_url, &statement).await?;
        Ok(Self { cluster, name })
    }

    /// 以维护账号连接：只用于迁移、DDL 夹具与必须越过运行账号权限的断言读取。
    pub async fn maintenance_store(&self) -> Result<DatabaseStore, String> {
        let url = with_database(&self.cluster.admin_url, &self.name)?;
        DatabaseStore::connect(&url, DatabaseConnectionProfile::legacy())
            .await
            .map_err(|error| format!("连接测试数据库失败：{error}"))
    }

    /// 默认连接：以运行账号身份连接该测试库，只继承 `auction_runtime` 的读写面，没有
    /// DDL 与 owner 权限。被测端口必须经它执行，才能证明生产运行账号的权限足够。
    pub async fn store(&self) -> Result<DatabaseStore, String> {
        let mut url = Url::parse(&with_database(&self.cluster.admin_url, &self.name)?)
            .map_err(|error| format!("解析测试数据库 URL 失败：{error}"))?;
        url.set_username(&self.cluster.service_role)
            .map_err(|()| "设置运行账号用户名失败".to_owned())?;
        url.set_password(Some(&self.cluster.service_password))
            .map_err(|()| "设置运行账号密码失败".to_owned())?;
        DatabaseStore::connect(url.as_str(), DatabaseConnectionProfile::legacy())
            .await
            .map_err(|error| format!("以运行账号连接测试数据库失败：{error}"))
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        drop_database(&self.cluster.admin_url, &self.name);
    }
}

/// 一次测试进程内共享的服务器与模板库。
struct Cluster {
    /// 具备建库/删库权限的维护连接；同时作为克隆库 URL 的基址。
    admin_url: String,
    template: String,
    /// 继承 `auction_runtime` 的一次性登录角色名与随机密码，只供 `service_store` 使用。
    service_role: String,
    service_password: String,
    server: Option<TempServer>,
}

impl Drop for Cluster {
    fn drop(&mut self) {
        if self.server.is_some() {
            // 一次性集群连同数据目录与角色一起删除，不需要再逐个清理。
            return;
        }
        drop_database(&self.admin_url, &self.template);
        drop_role(&self.admin_url, &self.service_role);
    }
}

/// 由本机二进制启动的一次性 PostgreSQL 18 集群。
struct TempServer {
    bin_dir: Option<PathBuf>,
    root: PathBuf,
    data_dir: PathBuf,
}

impl TempServer {
    fn start() -> Result<(String, Self), String> {
        let bin_dir = resolve_pg_bin()?;
        let root = env::temp_dir().join(unique_name("auction_pg18_"));
        let data_dir = root.join("data");
        let socket_dir = root.join("socket");
        std::fs::create_dir_all(&socket_dir)
            .map_err(|error| format!("创建临时集群目录失败：{error}"))?;

        let server = Self { bin_dir, root, data_dir };
        server.run_tool(
            "initdb",
            &[
                "-D",
                path_str(&server.data_dir)?,
                "-U",
                "postgres",
                "-A",
                "trust",
                "--no-sync",
                "--encoding=UTF8",
                "--locale=C",
            ],
            "initdb",
        )?;

        let port = free_port()?;
        let log_path = server.root.join("postgres.log");
        let options = format!("-p {port} -k {} -h 127.0.0.1 -F", path_str(&socket_dir)?);
        server.run_tool(
            "pg_ctl",
            &[
                "-D",
                path_str(&server.data_dir)?,
                "-l",
                path_str(&log_path)?,
                "-o",
                &options,
                "-w",
                "-t",
                "60",
                "start",
            ],
            "pg_ctl start",
        )?;

        let admin_url = format!("postgresql://postgres@127.0.0.1:{port}/postgres");
        Ok((admin_url, server))
    }

    fn tool(&self, name: &str) -> Command {
        match &self.bin_dir {
            Some(dir) => Command::new(dir.join(name)),
            None => Command::new(name),
        }
    }

    fn run_tool(&self, name: &str, args: &[&str], label: &str) -> Result<(), String> {
        let output = self
            .tool(name)
            .args(args)
            .output()
            .map_err(|error| format!("无法执行 {label}：{error}"))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("{label} 失败：{}", stderr.trim()))
    }
}

impl Drop for TempServer {
    fn drop(&mut self) {
        // 停止失败不覆盖测试结果；数据目录仍会被删除，残留进程由 pid 文件诊断。
        let _ = self
            .tool("pg_ctl")
            .args(["-D", self.data_dir.to_string_lossy().as_ref(), "-m", "immediate", "-w", "stop"])
            .output();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

async fn shared_cluster() -> Result<Arc<Cluster>, String> {
    let mut guard = CLUSTER.lock().await;
    if let Some(cluster) = guard.upgrade() {
        return Ok(cluster);
    }
    let cluster = Arc::new(bootstrap().await?);
    *guard = Arc::downgrade(&cluster);
    Ok(cluster)
}

async fn bootstrap() -> Result<Cluster, String> {
    let (admin_url, server) = match env::var("AUCTION_TEST_DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => (url.trim().to_owned(), None),
        _ => {
            let (url, server) = TempServer::start()?;
            (url, Some(server))
        }
    };
    ensure_server_major_18(&admin_url).await?;

    run_sql(
        &admin_url,
        "DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname='auction_runtime') THEN          CREATE ROLE auction_runtime LOGIN; END IF; END $$;",
    )
    .await?;
    let cluster = Cluster {
        admin_url,
        template: unique_name("auction_test_tmpl_"),
        service_role: unique_name("auction_test_svc_"),
        service_password: uuid::Uuid::new_v4().simple().to_string(),
        server,
    };
    let statement = format!("CREATE DATABASE {}", quote_ident(&cluster.template));
    run_sql(&cluster.admin_url, &statement).await?;
    prepare_template(&cluster).await?;
    // 只授予 auction_runtime 成员资格（默认 INHERIT），不带任何角色属性；外部服务器上要求维护角色
    // 对 auction_runtime 有 ADMIN OPTION（由它创建时 PG18 自动具备）。
    let statement = format!(
        "CREATE ROLE {} LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS \
         PASSWORD '{}' IN ROLE auction_runtime",
        quote_ident(&cluster.service_role),
        cluster.service_password
    );
    run_sql(&cluster.admin_url, &statement).await?;
    Ok(cluster)
}

/// 空模板库执行全部内嵌迁移，并核对内嵌最高版本确已应用。
async fn prepare_template(cluster: &Cluster) -> Result<(), String> {
    let url = with_database(&cluster.admin_url, &cluster.template)?;
    let store = DatabaseStore::connect(&url, DatabaseConnectionProfile::legacy())
        .await
        .map_err(|error| format!("连接模板库失败：{error}"))?;
    let migrated = store.migrate().await.map_err(|error| format!("模板库执行迁移失败：{error}"));
    let applied = match migrated {
        Ok(()) => sqlx::query_scalar::<_, i64>(
            "SELECT coalesce(max(version), 0) FROM public._sqlx_migrations",
        )
        .fetch_one(store.postgres_pool())
        .await
        .map_err(|error| format!("读取模板库迁移版本失败：{error}")),
        Err(error) => Err(error),
    };
    store.postgres_pool().close().await;

    let applied = applied?;
    let expected = postgres_expected_migration_version();
    if applied != expected {
        return Err(format!("模板库迁移版本 {applied} 不等于二进制内嵌迁移版本 {expected}"));
    }
    Ok(())
}

async fn ensure_server_major_18(admin_url: &str) -> Result<(), String> {
    let mut connection = PgConnection::connect(admin_url)
        .await
        .map_err(|error| format!("连接 PostgreSQL 测试服务器失败：{error}"))?;
    let version: String = sqlx::query_scalar("SELECT current_setting('server_version_num')")
        .fetch_one(&mut connection)
        .await
        .map_err(|error| format!("读取 PostgreSQL 服务器版本失败：{error}"))?;
    let _ = connection.close().await;
    let major = version
        .parse::<i32>()
        .map_err(|error| format!("无法解析 server_version_num={version}：{error}"))?
        / 10_000;
    if major != 18 {
        return Err(format!(
            "PostgreSQL 集成测试要求服务器主版本 18，实际 server_version_num={version}"
        ));
    }
    Ok(())
}

async fn run_sql(admin_url: &str, statement: &str) -> Result<(), String> {
    let mut connection = PgConnection::connect(admin_url)
        .await
        .map_err(|error| format!("连接 PostgreSQL 维护库失败：{error}"))?;
    let executed = sqlx::raw_sql(AssertSqlSafe(statement.to_owned()))
        .execute(&mut connection)
        .await
        .map_err(|error| format!("执行测试夹具 SQL 失败：{error}"));
    let closed = connection.close().await.map_err(|error| format!("关闭测试夹具连接失败：{error}"));
    executed?;
    closed
}

/// 在独立线程与独立 current-thread 运行时里删除数据库，避免在测试运行时上下文中阻塞或递归建 runtime。
fn drop_database(admin_url: &str, name: &str) {
    let admin_url = admin_url.to_owned();
    let name = name.to_owned();
    run_blocking_cleanup("auction-pg-drop", async move {
        let Ok(mut connection) = PgConnection::connect(&admin_url).await else {
            return;
        };
        // FORCE 终止克隆库上尚未释放的测试连接，保证清理不因连接池析构时序而失败。
        let statement = format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", quote_ident(&name));
        let _ = sqlx::raw_sql(AssertSqlSafe(statement)).execute(&mut connection).await;
        let _ = connection.close().await;
    });
}

/// 删除一次性运行账号角色；角色不拥有对象，删除失败不覆盖测试结果。
fn drop_role(admin_url: &str, role: &str) {
    let admin_url = admin_url.to_owned();
    let role = role.to_owned();
    run_blocking_cleanup("auction-pg-drop-role", async move {
        let Ok(mut connection) = PgConnection::connect(&admin_url).await else {
            return;
        };
        let statement = format!("DROP ROLE IF EXISTS {}", quote_ident(&role));
        let _ = sqlx::raw_sql(AssertSqlSafe(statement)).execute(&mut connection).await;
        let _ = connection.close().await;
    });
}

fn run_blocking_cleanup<F>(label: &str, future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    let spawned = std::thread::Builder::new().name(label.to_owned()).spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return;
        };
        runtime.block_on(future);
    });
    if let Ok(handle) = spawned {
        let _ = handle.join();
    }
}

fn resolve_pg_bin() -> Result<Option<PathBuf>, String> {
    if let Some(dir) = env::var_os("AUCTION_TEST_PG_BIN") {
        let dir = PathBuf::from(dir);
        return match pg_version(Some(&dir)) {
            Some(version) if version.starts_with("18.") => Ok(Some(dir)),
            Some(version) => {
                Err(format!("AUCTION_TEST_PG_BIN 指向 PostgreSQL {version}，集成测试要求 18"))
            }
            None => Err(format!("AUCTION_TEST_PG_BIN 下找不到可执行的 initdb：{}", dir.display())),
        };
    }
    let candidates = [None, Some(PathBuf::from("/usr/lib/postgresql/18/bin"))];
    for candidate in candidates {
        if let Some(version) = pg_version(candidate.as_ref())
            && version.starts_with("18.")
        {
            return Ok(candidate);
        }
    }
    Err(
        "找不到 PostgreSQL 18 二进制；请安装 postgresql-18 或设置 AUCTION_TEST_PG_BIN 指向其 bin 目录"
            .to_owned(),
    )
}

fn pg_version(dir: Option<&PathBuf>) -> Option<String> {
    let mut command = match dir {
        Some(dir) => Command::new(dir.join("initdb")),
        None => Command::new("initdb"),
    };
    let output = command.arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    // 形如 `initdb (PostgreSQL) 18.6 (Debian 18.6-1.pgdg13+2)`；取括号后的首个版本字段。
    let version = stdout.split(')').nth(1)?.split_whitespace().next()?;
    Some(version.to_owned())
}

fn free_port() -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("分配临时集群端口失败：{error}"))?;
    let port =
        listener.local_addr().map_err(|error| format!("读取临时集群端口失败：{error}"))?.port();
    drop(listener);
    Ok(port)
}

fn path_str(path: &Path) -> Result<&str, String> {
    path.to_str().ok_or_else(|| format!("测试夹具路径不是 UTF-8：{}", path.display()))
}

fn with_database(base: &str, database: &str) -> Result<String, String> {
    let mut url = Url::parse(base).map_err(|error| format!("解析数据库 URL 失败：{error}"))?;
    url.set_path(database);
    Ok(url.to_string())
}

fn quote_ident(identifier: &str) -> String {
    format!("\"{identifier}\"")
}

fn unique_name(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or_default();
    let counter = UNIQUE.fetch_add(1, Ordering::Relaxed);
    // PostgreSQL 标识符上限 63 字节；前缀加三段十六进制只保留低位，仍远低于上限。
    format!("{prefix}{:x}{nanos:x}{counter:x}", std::process::id())
}
