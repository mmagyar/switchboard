//! Owned append-only logging and filesystem change subscriptions.

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    thread::{self, ThreadId},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug)]
pub enum LogLevel {
    Log,
    Error,
    Warn,
    Info,
}

impl LogLevel {
    fn marker(self) -> char {
        match self {
            Self::Log => 'L',
            Self::Error => 'E',
            Self::Warn => 'W',
            Self::Info => 'I',
        }
    }
}

/// Clones share one serialized writer; dropping the last clone closes the file.
#[derive(Clone)]
pub struct FileLogger {
    file: Arc<Mutex<File>>,
    path: PathBuf,
}

pub fn init_file_logger(path: impl AsRef<Path>) -> io::Result<FileLogger> {
    let path = path.as_ref().to_owned();
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    Ok(FileLogger {
        file: Arc::new(Mutex::new(file)),
        path,
    })
}

impl FileLogger {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Values use the reference's single-value versus argument-array JSON encoding.
    pub fn log(&self, level: LogLevel, values: &[Value]) -> io::Result<()> {
        let content = if let [value] = values {
            crate::json::stringify(value)?
        } else {
            crate::json::stringify(&Value::Array(values.to_vec()))?
        };
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_millis();
        let mut file = self
            .file
            .lock()
            .map_err(|_| io::Error::other("log writer poisoned"))?;
        writeln!(file, "[{millis}][{}] - {content}", level.marker())?;
        file.flush()
    }
}

pub async fn read_logfile(path: impl AsRef<Path>, lines: usize) -> io::Result<String> {
    let path = path.as_ref().to_owned();
    tokio::task::spawn_blocking(move || {
        let reader = BufReader::new(File::open(path)?);
        let mut tail = VecDeque::new();
        for line in reader.lines() {
            let line = line?;
            if lines > 0 {
                if tail.len() == lines {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        }
        Ok(tail.into_iter().collect::<Vec<_>>().join("\n"))
    })
    .await
    .map_err(io::Error::other)?
}

/// The server closes subscriptions before joining its connection tasks.
pub trait LogWatcher: Send {
    fn close(&mut self);
}

type ChangeCallback = Arc<dyn Fn() + Send + Sync>;

struct CallbackState {
    callback: Option<ChangeCallback>,
    running: HashMap<ThreadId, usize>,
}

struct Callbacks {
    state: Mutex<CallbackState>,
    finished: Condvar,
}

struct RunningCallback(Arc<Callbacks>);

impl Drop for RunningCallback {
    fn drop(&mut self) {
        let mut state = self
            .0
            .state
            .lock()
            .expect("watcher callback state poisoned");
        let current = thread::current().id();
        if let Some(count) = state.running.get_mut(&current) {
            *count -= 1;
            if *count == 0 {
                state.running.remove(&current);
            }
        }
        self.0.finished.notify_all();
    }
}

impl Callbacks {
    fn invoke(self: &Arc<Self>) {
        let callback = {
            let mut state = self.state.lock().expect("watcher callback state poisoned");
            let Some(callback) = state.callback.clone() else {
                return;
            };
            *state.running.entry(thread::current().id()).or_default() += 1;
            callback
        };
        let _running = RunningCallback(self.clone());
        callback();
    }

    fn close(&self) {
        let current = thread::current().id();
        let mut state = self.state.lock().expect("watcher callback state poisoned");
        state.callback.take();
        // notify stops its event thread asynchronously. Join callbacks here, allowing self-close.
        while state.running.keys().any(|id| *id != current) {
            state = self
                .finished
                .wait(state)
                .expect("watcher callback state poisoned");
        }
    }
}

pub struct LogFileWatcher {
    watcher: Option<RecommendedWatcher>,
    callbacks: Arc<Callbacks>,
}

impl LogWatcher for LogFileWatcher {
    fn close(&mut self) {
        self.callbacks.close();
        self.watcher.take();
    }
}

impl Drop for LogFileWatcher {
    fn drop(&mut self) {
        self.close();
    }
}

pub fn log_file_change_watcher(
    path: impl AsRef<Path>,
    change: Arc<dyn Fn() + Send + Sync>,
) -> notify::Result<LogFileWatcher> {
    let callbacks = Arc::new(Callbacks {
        state: Mutex::new(CallbackState {
            callback: Some(change),
            running: HashMap::new(),
        }),
        finished: Condvar::new(),
    });
    let event_callbacks = callbacks.clone();
    let mut watcher =
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
            Ok(event) if matches!(event.kind, EventKind::Modify(_)) => event_callbacks.invoke(),
            Err(error) => tracing::warn!(%error, "log watcher failed"),
            _ => {}
        })?;
    watcher.watch(path.as_ref(), RecursiveMode::NonRecursive)?;
    Ok(LogFileWatcher {
        watcher: Some(watcher),
        callbacks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;
    use tokio::time::{Duration, timeout};

    #[tokio::test]
    async fn file_logger_appends_json_and_reads_bounded_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        let logger = init_file_logger(&path).unwrap();
        logger
            .log(LogLevel::Info, &[Value::String("hello".into())])
            .unwrap();
        logger
            .log(LogLevel::Warn, &[Value::from(1), Value::from(false)])
            .unwrap();
        drop(logger);
        let logger = init_file_logger(&path).unwrap();
        logger.log(LogLevel::Error, &[]).unwrap();
        let all = read_logfile(&path, 80).await.unwrap();
        let lines: Vec<_> = all.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].ends_with("][I] - \"hello\""));
        assert!(lines[1].ends_with("][W] - [1,false]"));
        assert!(lines[2].ends_with("][E] - []"));
        assert_eq!(read_logfile(&path, 1).await.unwrap(), lines[2]);
        assert_eq!(read_logfile(&path, 0).await.unwrap(), "");
        assert_eq!(
            read_logfile(dir.path().join("missing"), 80)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }

    #[tokio::test]
    async fn watcher_can_close_itself_from_its_callback() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        let logger = init_file_logger(&path).unwrap();
        let holder: Arc<Mutex<Option<LogFileWatcher>>> = Arc::new(Mutex::new(None));
        let callback_holder = holder.clone();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let watcher = log_file_change_watcher(
            &path,
            Arc::new(move || {
                if let Some(watcher) = callback_holder.lock().unwrap().as_mut() {
                    watcher.close();
                }
                tx.send(()).unwrap();
            }),
        )
        .unwrap();
        *holder.lock().unwrap() = Some(watcher);
        logger
            .log(LogLevel::Info, &[Value::from("changed")])
            .unwrap();
        assert_eq!(
            timeout(Duration::from_secs(2), rx.recv()).await.unwrap(),
            Some(())
        );
        holder.lock().unwrap().take();
        assert_eq!(
            timeout(Duration::from_secs(2), rx.recv()).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn watcher_observes_writes_and_close_releases_callback() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        let logger = init_file_logger(&path).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut watcher = log_file_change_watcher(
            &path,
            Arc::new(move || {
                let _ = tx.send(());
            }),
        )
        .unwrap();
        logger
            .log(LogLevel::Info, &[Value::from("changed")])
            .unwrap();
        assert_eq!(
            timeout(Duration::from_secs(2), rx.recv()).await.unwrap(),
            Some(())
        );
        watcher.close();
        while rx.try_recv().is_ok() {}
        assert_eq!(
            timeout(Duration::from_secs(2), rx.recv()).await.unwrap(),
            None
        );
        watcher.close();
    }
}
