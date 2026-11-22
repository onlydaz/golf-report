//! Best-effort diagnostics only; never participates in PLC/DB decisions.
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fmt,
    fs::{self, File, OpenOptions},
    future::Future,
    io::{self, Write},
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock, mpsc},
};

// Task-local, not thread-local/global: concurrent projection or inspection must
// never borrow another task's Inbox identity. No context is used by ACK guards.
#[derive(Clone, Debug, Default)]
pub(crate) struct Context {
    pub source: String,
    pub session: String,
    pub endpoint: String,
    pub inbox: Option<i64>,
    pub measurement: Option<i64>,
    pub target: Option<i32>,
    pub record_seq: Option<i32>,
    pub phase: &'static str,
    pub checkpoint: String,
    pub control: String,
    pub count_call: &'static str,
    pub done_call: &'static str,
}

tokio::task_local! {
    static CONTEXT: RefCell<Context>;
}

pub(crate) async fn scope<T>(context: Context, future: impl Future<Output = T>) -> T {
    CONTEXT.scope(RefCell::new(context), future).await
}

pub(crate) fn update(change: impl FnOnce(&mut Context)) {
    let _ = CONTEXT.try_with(|context| change(&mut context.borrow_mut()));
}

macro_rules! event {
    ($($arg:tt)*) => { $crate::plc::diagnostics::emit(format_args!($($arg)*)) };
}
pub(crate) use event;

macro_rules! changed_event {
    ($key:expr, $($arg:tt)*) => { $crate::plc::diagnostics::emit_changed($key, format_args!($($arg)*)) };
}
pub(crate) use changed_event;

pub(crate) fn emit_changed(key: &'static str, arguments: fmt::Arguments<'_>) {
    emit_changed_signature(key, format!("{arguments}"), arguments);
}

fn changed_events() -> &'static Mutex<HashMap<&'static str, String>> {
    static PREVIOUS: OnceLock<Mutex<HashMap<&'static str, String>>> = OnceLock::new();
    PREVIOUS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn forget_changed(key: &'static str) {
    if let Ok(mut previous) = changed_events().lock() { previous.remove(key); }
}

pub(crate) fn emit_changed_signature(key: &'static str, signature: String, arguments: fmt::Arguments<'_>) {
    let context = CONTEXT.try_with(|context| format!("{:?}", context.borrow())).unwrap_or_default();
    let signature = format!("{signature}{context}");
    let changed = changed_events().lock().map(|mut previous| {
        let changed = previous.get(key) != Some(&signature);
        if changed { previous.insert(key, signature); }
        changed
    }).unwrap_or(true);
    if changed || debug_enabled() { emit(arguments); }
}

pub(crate) fn write_outcome(function: &str, outcome: &'static str) {
    update(|context| {
        if function == "FC16" {
            context.count_call = outcome;
            context.phase = if outcome == "CALL_START" { "FC16_CALL_START" } else { "FC16_RESPONSE" };
        } else {
            context.done_call = outcome;
            context.phase = if outcome == "CALL_START" { "FC06_CALL_START" } else { "FC06_RESPONSE" };
        }
    });
}

/// Never serialize connection options, SQL parameters or server DETAIL (which
/// may contain row values). SQLSTATE/constraint + operation locate the failure.
pub(crate) fn sql_failure(operation: &'static str, error: &sqlx::Error) {
    match error {
        sqlx::Error::Database(database) => changed_event!(operation, "PLC_ACK_DATABASE stage=SQL_ERROR operation={operation} sqlstate={:?} constraint={:?} table={:?} commit_or_update_not_confirmed=true",
            database.code(), database.constraint(), database.table()),
        _ => changed_event!(operation, "PLC_ACK_DATABASE stage=SQL_ERROR operation={operation} error_kind={} commit_or_update_not_confirmed=true",
            match error {
                sqlx::Error::Io(_) => "IO",
                sqlx::Error::Tls(_) => "TLS",
                sqlx::Error::PoolTimedOut => "POOL_TIMEOUT",
                sqlx::Error::PoolClosed => "POOL_CLOSED",
                sqlx::Error::RowNotFound => "ROW_NOT_FOUND",
                sqlx::Error::Protocol(_) => "PROTOCOL",
                _ => "OTHER_SQLX_ERROR",
            }),
    }
}

fn terminal(line: &str) {
    // A closed terminal/log pipe must not panic inside the acquisition task.
    let _ = writeln!(io::stderr().lock(), "{line}");
}

pub(crate) fn emit(arguments: fmt::Arguments<'_>) {
    let context = CONTEXT.try_with(|context| {
        let c = context.borrow();
        format!(" ack_id={:?} source={:?} session={:?} endpoint={:?} inbox={:?} measurement_id={:?} record_seq={:?} target={:?} phase={} checkpoint=[{}] last_successful_fc03=[{}] count_call_this_poll={} done_call_this_poll={}",
            format!("{}/{}/{:?}", c.source, c.session, c.target), c.source, c.session,
            c.endpoint, c.inbox, c.measurement, c.record_seq, c.target, c.phase,
            c.checkpoint, c.control, c.count_call, c.done_call)
    }).unwrap_or_default();
    let line = format!("timestamp={} {}{}", chrono::Utc::now().to_rfc3339(), arguments, context);
    terminal(&line);
    if let Some(sink) = FILE_SINK.get() {
        // Never wait for disk/rotation from the Worker. Terminal keeps every
        // emitted event even if the bounded file queue cannot accept it.
        if let Err(error) = sink.try_send(FileRecord::Line(line)) {
            static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                terminal(&format!("timestamp={} PLC_DIAGNOSTICS state=FILE_QUEUE_UNAVAILABLE reason={error} terminal_logging_retained=true", chrono::Utc::now().to_rfc3339()));
            }
        }
    }
}

const FILE_LIMIT: u64 = 4 * 1024 * 1024;
const ARCHIVES: usize = 5;
enum FileRecord {
    Line(String),
    Flush(mpsc::Sender<()>),
}
static FILE_SINK: OnceLock<mpsc::SyncSender<FileRecord>> = OnceLock::new();

/// Called only at application exit, never by the Worker or ACK path.
pub(crate) fn flush_on_exit() {
    if let Some(sink) = FILE_SINK.get() {
        let (sender, receiver) = mpsc::channel();
        let queued = sink.try_send(FileRecord::Flush(sender)).is_ok();
        if !queued || receiver.recv_timeout(std::time::Duration::from_millis(500)).is_err() {
            terminal("PLC_DIAGNOSTICS state=EXIT_FLUSH_UNCONFIRMED pending_file_lines_may_be_missing=true");
        }
    }
}

// Outside local-runtime: log files are NOT legacy acquisition evidence.
// Append across restarts; only size rotation removes the oldest log archive.
pub(crate) fn initialize(directory: &Path) {
    if FILE_SINK.get().is_some() {
        return;
    }
    let directory = directory.join("logs");
    let (sender, receiver) = mpsc::sync_channel::<FileRecord>(8192);
    let path = directory.join("plc-ack.log");
    let display_path = path.clone();
    let result = std::thread::Builder::new().name("plc-diagnostics".into()).spawn(move || {
        let mut file: Option<(File, u64)> = None;
        let mut failed = false;
        while let Ok(record) = receiver.recv() {
            let line = match record {
                FileRecord::Line(line) => line,
                FileRecord::Flush(acknowledge) => {
                    let flushed = match file.as_ref() {
                        Some((file, _)) => file.sync_all().is_ok(),
                        None => !failed,
                    };
                    if flushed { let _ = acknowledge.send(()); }
                    continue;
                }
            };
            let result = (|| -> io::Result<()> {
                if file.is_none() {
                    fs::create_dir_all(&directory)?;
                    let opened = OpenOptions::new().create(true).append(true).open(&path)?;
                    let size = opened.metadata()?.len();
                    file = Some((opened, size));
                }
                if file.as_ref().is_some_and(|(_, size)| *size + line.len() as u64 + 1 > FILE_LIMIT) {
                    file.take(); // Close before renaming, including on Windows.
                    rotate(&path)?;
                    let opened = OpenOptions::new().create(true).append(true).open(&path)?;
                    let size = opened.metadata()?.len();
                    file = Some((opened, size));
                }
                if let Some((file, size)) = file.as_mut() {
                    // File is unbuffered at the Rust level; lines remain readable
                    // without relying on a BufWriter destructor at process exit.
                    writeln!(file, "{line}")?;
                    *size += line.len() as u64 + 1;
                }
                Ok(())
            })();
            match result {
                Ok(()) => {
                    if failed {
                        terminal(&format!("timestamp={} PLC_DIAGNOSTICS state=FILE_RECOVERED path={}", chrono::Utc::now().to_rfc3339(), path.display()));
                    }
                    failed = false;
                }
                Err(error) => {
                    file.take();
                    if !failed {
                        terminal(&format!("timestamp={} PLC_DIAGNOSTICS state=FILE_ERROR path={} error={error} worker_unaffected=true terminal_logging_retained=true", chrono::Utc::now().to_rfc3339(), path.display()));
                    }
                    failed = true;
                }
            }
        }
    });
    match result {
        Ok(_) => {
            if FILE_SINK.set(sender).is_ok() {
                event!("PLC_DIAGNOSTICS state=STARTED path={} file_limit_bytes={FILE_LIMIT} archives={ARCHIVES} debug={} persistence=best_effort_nonblocking", display_path.display(), debug_enabled());
            }
        }
        Err(error) => event!("PLC_DIAGNOSTICS state=THREAD_ERROR error={error} terminal_logging_retained=true"),
    }
}

fn archive(path: &Path, index: usize) -> PathBuf {
    path.with_file_name(format!("plc-ack.{index}.log"))
}

fn rotate(path: &Path) -> io::Result<()> {
    let oldest = archive(path, ARCHIVES);
    if oldest.exists() {
        fs::remove_file(oldest)?;
    }
    for index in (1..ARCHIVES).rev() {
        let old = archive(path, index);
        if old.exists() {
            fs::rename(old, archive(path, index + 1))?;
        }
    }
    fs::rename(path, archive(path, 1))
}

/// Opt in before starting the process. No logging dependency or TOML change.
pub(super) fn debug_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("GOLF_REPORT_PLC_DEBUG").is_ok_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "debug" | "trace"
            )
        })
    })
}

type TcpSource = (SocketAddr, Option<IpAddr>);
fn failed_tcp_sources() -> &'static Mutex<HashSet<TcpSource>> {
    static FAILED: OnceLock<Mutex<HashSet<TcpSource>>> = OnceLock::new();
    FAILED.get_or_init(|| Mutex::new(HashSet::new()))
}

pub(super) fn note_tcp_failure(endpoint: SocketAddr, source: Option<IpAddr>) {
    if let Ok(mut failed) = failed_tcp_sources().lock() {
        failed.insert((endpoint, source));
    }
}

/// Only a failed open for this endpoint/source makes the next open a recovery.
/// A new ephemeral port by itself never counts as a reconnect.
pub(super) fn take_tcp_failure(endpoint: SocketAddr, source: Option<IpAddr>) -> bool {
    failed_tcp_sources()
        .lock()
        .map(|mut failed| failed.remove(&(endpoint, source)))
        .unwrap_or(true)
}

/// One last value per fixed event name, not an ever-growing publication history.
#[derive(Default)]
pub(super) struct ChangedLogs {
    previous: HashMap<&'static str, String>,
}

impl ChangedLogs {
    pub(super) fn should_emit(&mut self, event: &'static str, value: String) -> bool {
        let changed = self.previous.get(event) != Some(&value);
        if changed {
            self.previous.insert(event, value);
        }
        changed || debug_enabled()
    }

    pub(super) fn forget(&mut self, event: &'static str) -> bool {
        self.previous.remove(event).is_some()
    }

    pub(super) fn clear(&mut self) {
        self.previous.clear();
    }
}
