use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    panic,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const LOG_FILE_NAME: &str = "webcam-control.jsonl";
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
const RETAINED_LOGS: usize = 5;
const MAX_FIELD_CHARS: usize = 16 * 1024;

struct Diagnostics {
    directory: PathBuf,
    session_id: String,
    started: Instant,
    writer: Mutex<()>,
}

static DIAGNOSTICS: OnceLock<Diagnostics> = OnceLock::new();

pub fn init(directory: PathBuf) -> Result<PathBuf, String> {
    fs::create_dir_all(&directory)
        .map_err(|error| format!("No se pudo crear la carpeta de diagnóstico: {error}"))?;
    let timestamp = timestamp_millis();
    let session_id = format!("{timestamp}-{}", std::process::id());
    let diagnostics = Diagnostics {
        directory: directory.clone(),
        session_id,
        started: Instant::now(),
        writer: Mutex::new(()),
    };
    DIAGNOSTICS
        .set(diagnostics)
        .map_err(|_| "El diagnóstico ya estaba inicializado.".to_string())?;
    Ok(directory.join(LOG_FILE_NAME))
}

pub fn directory() -> Option<PathBuf> {
    DIAGNOSTICS.get().map(|state| state.directory.clone())
}

pub fn install_panic_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|value| format!("{}:{}:{}", value.file(), value.line(), value.column()));
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|value| (*value).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "Pánico sin mensaje".to_string());
        log(
            "fatal",
            "rust",
            "panic",
            &message,
            json!({ "location": location, "thread": thread_name() }),
        );
        previous(info);
    }));
}

pub fn log(level: &str, subsystem: &str, event: &str, message: &str, context: Value) {
    let Some(state) = DIAGNOSTICS.get() else {
        return;
    };
    let Ok(_guard) = state.writer.lock() else {
        return;
    };
    let path = state.directory.join(LOG_FILE_NAME);
    if path.metadata().map(|value| value.len()).unwrap_or(0) >= MAX_LOG_BYTES {
        rotate(&state.directory);
    }
    let record = json!({
        "timestampMs": timestamp_millis(),
        "uptimeMs": state.started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        "sessionId": state.session_id,
        "pid": std::process::id(),
        "thread": thread_name(),
        "level": normalize_level(level),
        "subsystem": truncate(subsystem),
        "event": truncate(event),
        "message": truncate(message),
        "context": limit_value(context, 0),
    });
    if let Ok(mut file) = open_log(&path) {
        if serde_json::to_writer(&mut file, &record).is_ok() {
            let _ = file.write_all(b"\n");
            let _ = file.flush();
        }
    }
}

pub fn ingest_external_line(subsystem: &str, line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(Value::Object(mut object)) => {
            let level = object
                .remove("level")
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "debug".to_string());
            let event = object
                .remove("event")
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "external".to_string());
            let message = object
                .remove("message")
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default();
            log(&level, subsystem, &event, &message, Value::Object(object));
        }
        _ => log("debug", subsystem, "stderr", trimmed, Value::Null),
    }
}

fn open_log(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

fn rotate(directory: &Path) {
    let oldest = directory.join(format!("{LOG_FILE_NAME}.{RETAINED_LOGS}"));
    let _ = fs::remove_file(oldest);
    for index in (1..RETAINED_LOGS).rev() {
        let source = directory.join(format!("{LOG_FILE_NAME}.{index}"));
        let target = directory.join(format!("{LOG_FILE_NAME}.{}", index + 1));
        if source.exists() {
            let _ = fs::rename(source, target);
        }
    }
    let current = directory.join(LOG_FILE_NAME);
    if current.exists() {
        let _ = fs::rename(current, directory.join(format!("{LOG_FILE_NAME}.1")));
    }
}

fn timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn thread_name() -> String {
    std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_string()
}

fn normalize_level(level: &str) -> &'static str {
    match level.to_ascii_lowercase().as_str() {
        "trace" => "trace",
        "debug" => "debug",
        "warn" | "warning" => "warn",
        "error" => "error",
        "fatal" => "fatal",
        _ => "info",
    }
}

fn truncate(value: &str) -> String {
    let mut chars = value.chars();
    let shortened: String = chars.by_ref().take(MAX_FIELD_CHARS).collect();
    if chars.next().is_some() {
        format!("{shortened}…[truncado]")
    } else {
        shortened
    }
}

fn limit_value(value: Value, depth: usize) -> Value {
    if depth >= 8 {
        return Value::String("[profundidad limitada]".to_string());
    }
    match value {
        Value::String(value) => Value::String(truncate(&value)),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .take(100)
                .map(|value| limit_value(value, depth + 1))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .take(100)
                .map(|(key, value)| (truncate(&key), limit_value(value, depth + 1)))
                .collect(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_large_fields() {
        let value = "x".repeat(MAX_FIELD_CHARS + 20);
        assert!(truncate(&value).ends_with("…[truncado]"));
    }

    #[test]
    fn normalizes_unknown_levels() {
        assert_eq!(normalize_level("WARNING"), "warn");
        assert_eq!(normalize_level("anything"), "info");
    }
}
