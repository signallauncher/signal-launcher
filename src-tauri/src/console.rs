//! Game console: reads the game's stdout/stderr, writes the full session log, keeps the last
//! `CAPACITY` lines per installation and streams new lines to the UI in batches (`game-log`),
//! so a mod that spams thousands of lines a second cannot flood the WebView.
//! When the game crashes, `diagnose` looks for the likely cause.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::Serialize;

/// Lines kept per installation (Rust side and UI side).
pub const CAPACITY: usize = 5000;
/// At most this many lines per UI update; the rest is counted as skipped (it stays in the file).
const BATCH_MAX: usize = 400;
const FLUSH_EVERY: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    /// Line number in the session, from 1.
    pub n: u64,
    /// `info`, `warn`, `error`, `fatal`, `trace` (stack trace), `debug`.
    pub level: &'static str,
    pub text: String,
    /// Came from stderr.
    pub err: bool,
}

/// One UI update: `game-log`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Batch {
    pub instance_id: String,
    pub lines: Vec<LogLine>,
    /// Lines that were not sent in this batch (spam); they are in the session log file.
    pub skipped: u64,
}

/// Why the game probably crashed: shown in the console and in the crash toast.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CrashReason {
    pub title: String,
    pub hint: String,
    /// The line that gives it away, if any.
    pub detail: Option<String>,
    /// Its number in the session, to scroll to.
    pub line: Option<u64>,
}

/// Sessions by installation id (managed by Tauri).
#[derive(Default)]
pub struct Console {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

impl Console {
    pub fn snapshot(&self, instance_id: &str) -> Vec<LogLine> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.get(instance_id).map(|s| s.snapshot()).unwrap_or_default()
    }
}

#[derive(Default)]
pub struct Session {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    ring: VecDeque<LogLine>,
    pending: Vec<LogLine>,
    next: u64,
    skipped: u64,
    last_level: &'static str,
    file: Option<File>,
}

impl Session {
    fn push(&self, raw: &[u8], err: bool) {
        let text = String::from_utf8_lossy(raw).trim_end_matches(['\r', '\n']).to_string();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(file) = inner.file.as_mut() {
            let _ = writeln!(file, "{text}");
        }
        let level = classify(&text, err, inner.last_level);
        inner.last_level = level;
        inner.next += 1;
        let line = LogLine { n: inner.next, level, text, err };
        if inner.ring.len() == CAPACITY {
            inner.ring.pop_front();
        }
        inner.ring.push_back(line.clone());
        inner.pending.push(line);
    }

    /// New lines since the last call: the newest `BATCH_MAX`, plus how many were skipped.
    fn take_batch(&self) -> Option<(Vec<LogLine>, u64)> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.pending.is_empty() && inner.skipped == 0 {
            return None;
        }
        let mut lines = std::mem::take(&mut inner.pending);
        if lines.len() > BATCH_MAX {
            inner.skipped += (lines.len() - BATCH_MAX) as u64;
            lines.drain(..lines.len() - BATCH_MAX);
        }
        let skipped = std::mem::take(&mut inner.skipped);
        Some((lines, skipped))
    }

    pub fn snapshot(&self) -> Vec<LogLine> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.ring.iter().cloned().collect()
    }
}

/// A running session: reader threads plus the batch sender.
pub struct Attached {
    session: Arc<Session>,
    readers: Vec<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    flusher: JoinHandle<()>,
}

impl Attached {
    /// Waits until both streams are read to the end and the last batch is sent.
    pub fn finish(self) -> Arc<Session> {
        for reader in self.readers {
            let _ = reader.join();
        }
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.flusher.join();
        if let Some(file) = self.session.inner.lock().unwrap_or_else(|e| e.into_inner()).file.as_mut() {
            let _ = file.flush();
        }
        self.session
    }
}

/// Starts reading `streams` (`(reader, is_stderr)`) into a fresh session for `instance_id`.
/// `emit` receives a batch roughly every 100 ms while there is output.
pub fn attach(
    console: &Console,
    instance_id: &str,
    streams: Vec<(Box<dyn Read + Send>, bool)>,
    log_file: Option<File>,
    emit: impl Fn(Batch) + Send + 'static,
) -> Attached {
    let session = Arc::new(Session::default());
    session.inner.lock().unwrap_or_else(|e| e.into_inner()).file = log_file;
    console
        .sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(instance_id.to_string(), session.clone());

    let readers = streams
        .into_iter()
        .map(|(stream, err)| {
            let session = session.clone();
            std::thread::spawn(move || {
                // Split on bytes: game output is not always valid UTF-8.
                for line in BufReader::new(stream).split(b'\n').map_while(Result::ok) {
                    session.push(&line, err);
                }
            })
        })
        .collect();

    let stop = Arc::new(AtomicBool::new(false));
    let flusher = {
        let session = session.clone();
        let stop = stop.clone();
        let instance_id = instance_id.to_string();
        std::thread::spawn(move || loop {
            let last_round = stop.load(Ordering::SeqCst);
            if let Some((lines, skipped)) = session.take_batch() {
                emit(Batch { instance_id: instance_id.clone(), lines, skipped });
            }
            if last_round {
                break;
            }
            std::thread::sleep(FLUSH_EVERY);
        })
    };
    Attached { session, readers, stop, flusher }
}

// ---------------------------------------------------------------------------------------------
// Levels
// ---------------------------------------------------------------------------------------------

/// Level of one output line. Understands log4j (`[12:00:00] [Render thread/WARN]: …`), Forge's
/// `[main/INFO] [mod/]:`, bare `[WARN]`, Java stack traces and JVM crash banners.
fn classify(text: &str, err: bool, prev: &'static str) -> &'static str {
    let t = text.trim_start();
    let indented = text.starts_with('\t') || text.starts_with("  ");
    if (indented && t.starts_with("at ")) || t.starts_with("Caused by:") || t.starts_with("Suppressed:") || (t.starts_with("... ") && t.ends_with(" more")) {
        return "trace";
    }
    if let Some(level) = bracket_level(text) {
        return level;
    }
    if t.starts_with("# A fatal error has been detected") || (t.starts_with('#') && prev == "fatal") {
        return "fatal";
    }
    if t.starts_with("Exception in thread") || is_exception_header(t) {
        return "error";
    }
    if err {
        "warn"
    } else {
        "info"
    }
}

fn bracket_level(text: &str) -> Option<&'static str> {
    let head: String = text.chars().take(160).collect();
    let upper = head.to_ascii_uppercase();
    for (tag, level) in [
        ("FATAL]", "fatal"),
        ("ERROR]", "error"),
        ("SEVERE]", "error"),
        ("WARN]", "warn"),
        ("WARNING]", "warn"),
        ("INFO]", "info"),
        ("DEBUG]", "debug"),
        ("TRACE]", "debug"),
    ] {
        if upper.contains(&format!("/{tag}")) || upper.contains(&format!("[{tag}")) {
            return Some(level);
        }
    }
    None
}

/// `java.lang.IllegalStateException: …`, `net.minecraft.ReportedException`
fn is_exception_header(t: &str) -> bool {
    let head = t.split(':').next().unwrap_or(t);
    !head.contains(' ') && head.contains('.') && (head.ends_with("Exception") || head.ends_with("Error"))
}

// ---------------------------------------------------------------------------------------------
// Crash detector
// ---------------------------------------------------------------------------------------------

/// Looks through the session (and the crash report, if Minecraft wrote one) for the usual
/// reasons: JVM crash, out of memory, wrong Java, missing / conflicting mods, video driver.
pub fn diagnose(lines: &[LogLine], crash_report: Option<&str>) -> CrashReason {
    let find = |needles: &[&str]| lines.iter().find(|l| needles.iter().any(|n| l.text.contains(n)));
    let reason = |title: &str, hint: &str, line: Option<&LogLine>| CrashReason {
        title: title.into(),
        hint: hint.into(),
        detail: line.map(|l| l.text.trim().chars().take(300).collect()),
        line: line.map(|l| l.n),
    };

    if let Some(l) = find(&["A fatal error has been detected by the Java Runtime Environment"]) {
        return reason(
            "Сбой виртуальной машины Java",
            "Чаще всего виноват видеодрайвер или нестабильный разгон. Обновите драйвер видеокарты. Подробный отчёт JVM — файл hs_err_pid*.log в папке установки.",
            Some(l),
        );
    }
    if let Some(l) = find(&["java.lang.OutOfMemoryError"]) {
        return reason(
            "Игре не хватило памяти",
            "Выделите больше ОЗУ в «Настройки → Игра и Графика» или уберите тяжёлые моды и шейдеры.",
            Some(l),
        );
    }
    if let Some(l) = find(&["UnsupportedClassVersionError", "Unsupported class file major version", "compiled by a more recent version of the Java Runtime"]) {
        return reason(
            "Нужна более новая Java",
            "Какой-то мод собран под более новую Java, чем требует эта версия игры. Обновите мод или возьмите версию, подходящую к вашей версии Minecraft.",
            Some(l),
        );
    }
    if let Some(l) = find(&["Found duplicate mods", "DuplicateModsFoundException", "Duplicate mods found", "is loaded from multiple"]) {
        return reason("Один мод установлен дважды", "Удалите лишнюю копию мода во вкладке «Моды и Сборки → Установленные».", Some(l));
    }
    if let Some(l) = find(&[
        "Incompatible mods found",
        "Incompatible mod set",
        "Could not find required mod",
        "Missing or unsupported mandatory dependencies",
        "requires any version of",
        "requires version",
        "Mod resolution failed",
    ]) {
        // Fabric prints the useful line just after the header.
        let detail_line = lines
            .iter()
            .skip_while(|x| x.n != l.n)
            .skip(1)
            .find(|x| x.text.contains("requires") || x.text.contains("Mod '") || x.text.contains("mod "))
            .unwrap_or(l);
        return reason(
            "Не хватает зависимости или моды несовместимы",
            "Установите недостающий мод или уберите несовместимый. Точное имя — в строке ниже.",
            Some(detail_line),
        );
    }
    if let Some(l) = find(&[
        "Mixin apply failed",
        "MixinApplyError",
        "InvalidInjectionException",
        "MixinTransformerError",
        "Mixin transformation of",
        "failed injection check",
        "Critical injection failure",
        "MixinPreProcessorException",
    ]) {
        // «Mixin [iris.mixins.json:MixinLevelRenderer] from mod iris failed …»
        let words: Vec<&str> = l
            .text
            .split(|c: char| c.is_whitespace() || "'\"()[]:,".contains(c))
            .filter(|w| !w.is_empty())
            .collect();
        let config = words.iter().find(|w| w.ends_with(".json") && w.contains("mixins"));
        let module = words.windows(3).find(|w| w[0] == "from" && w[1] == "mod").map(|w| w[2]);
        let config = match (module, config) {
            (Some(m), Some(c)) => format!(" (мод {m}, {c})"),
            (Some(m), None) => format!(" (мод {m})"),
            (None, Some(c)) => format!(" (конфигурация {c})"),
            (None, None) => String::new(),
        };
        return reason(
            "Конфликт модов (Mixin)",
            &format!("Два мода меняют один и тот же код игры{config}. Отключите недавно добавленные моды по одному, чтобы найти пару."),
            Some(l),
        );
    }
    if let Some(l) = find(&["GLFW error 65542", "WGL: The driver does not appear to support OpenGL", "Pixel format not accelerated", "Failed to create the GLFW window", "Couldn't set pixel format"]) {
        return reason(
            "Видеодрайвер не поддерживает OpenGL",
            "Установите драйвер видеокарты с сайта производителя (NVIDIA, AMD, Intel). Стандартный драйвер Windows OpenGL не умеет.",
            Some(l),
        );
    }
    if let Some(report) = crash_report {
        let mut rows = report.lines();
        if let Some(desc) = rows.by_ref().find_map(|r| r.strip_prefix("Description:")) {
            let exception = rows.map(str::trim).find(|r| !r.is_empty()).unwrap_or("");
            let in_log = lines.iter().rev().find(|x| !exception.is_empty() && x.text.contains(exception));
            let mut found = reason(
                &format!("Игра упала: {}", desc.trim()),
                "Причина — в первой строке исключения ниже и в отчёте о сбое.",
                in_log,
            );
            if found.detail.is_none() && !exception.is_empty() {
                found.detail = Some(exception.chars().take(300).collect());
            }
            return found;
        }
    }
    let last_error = lines
        .iter()
        .rev()
        .find(|l| (l.level == "error" || l.level == "fatal") && (l.text.contains("Exception") || l.text.contains("Error")));
    reason(
        "Игра завершилась с ошибкой",
        if last_error.is_some() { "Последняя ошибка в журнале — ниже." } else { "В журнале нет явной ошибки. Проверьте отчёт о сбое и последние строки консоли." },
        last_error,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(texts: &[&str]) -> Vec<LogLine> {
        let mut prev = "info";
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let level = classify(t, false, prev);
                prev = level;
                LogLine { n: i as u64 + 1, level, text: t.to_string(), err: false }
            })
            .collect()
    }

    #[test]
    fn classifies_levels() {
        assert_eq!(classify("[12:00:01] [Render thread/WARN]: Missing sound", false, "info"), "warn");
        assert_eq!(classify("[12:00:01] [main/INFO] [cp.mo.mo.Launcher/MODLAUNCHER]: ModLauncher running", false, "info"), "info");
        assert_eq!(classify("[12:00:02] [Render thread/ERROR]: Failed", false, "info"), "error");
        assert_eq!(classify("[12:00:02] [main/FATAL]: Boom", false, "info"), "fatal");
        assert_eq!(classify("java.lang.NullPointerException: Cannot invoke", false, "info"), "error");
        assert_eq!(classify("\tat net.minecraft.client.Main.main(Main.java:42)", false, "error"), "trace");
        assert_eq!(classify("Caused by: java.io.IOException", false, "trace"), "trace");
        assert_eq!(classify("\t... 12 more", false, "trace"), "trace");
        assert_eq!(classify("# A fatal error has been detected by the Java Runtime Environment:", true, "info"), "fatal");
        assert_eq!(classify("#  EXCEPTION_ACCESS_VIOLATION (0xc0000005)", true, "fatal"), "fatal");
        assert_eq!(classify("some text on stderr", true, "info"), "warn");
        assert_eq!(classify("Loading 42 mods", false, "info"), "info");
    }

    #[test]
    fn diagnoses_common_crashes() {
        let oom = diagnose(&lines(&["[main/INFO]: ok", "java.lang.OutOfMemoryError: Java heap space"]), None);
        assert_eq!(oom.title, "Игре не хватило памяти");
        assert_eq!(oom.line, Some(2));

        let deps = diagnose(
            &lines(&[
                "[main/ERROR]: Incompatible mods found!",
                "net.fabricmc.loader.impl.FormattedException: Some of your mods are incompatible",
                " - Mod 'Sodium Extra' (sodium-extra) requires any version of sodium, which is missing!",
            ]),
            None,
        );
        assert_eq!(deps.title, "Не хватает зависимости или моды несовместимы");
        assert!(deps.detail.unwrap().contains("requires any version of sodium"));

        let mixin = diagnose(&lines(&["org.spongepowered.asm.mixin.transformer.throwables.MixinTransformerError: Mixin apply failed iris.mixins.json:MixinLevelRenderer"]), None);
        assert!(mixin.hint.contains("iris.mixins.json"));
        let mixin = diagnose(&lines(&["Mixin [sodium.mixins.json:core.MixinWindow] from mod sodium failed injection check"]), None);
        assert!(mixin.hint.contains("мод sodium, sodium.mixins.json"), "{}", mixin.hint);

        let report = "---- Minecraft Crash Report ----\nTime: now\nDescription: Ticking entity\n\njava.lang.NullPointerException: boom\n\tat x.y(z)";
        let crash = diagnose(&lines(&["[Server thread/INFO]: Saving"]), Some(report));
        assert_eq!(crash.title, "Игра упала: Ticking entity");
        assert_eq!(crash.detail.as_deref(), Some("java.lang.NullPointerException: boom"));
    }

    #[test]
    fn batches_spam_and_keeps_the_ring() {
        let console = Console::default();
        let spam: String = (1..=CAPACITY + 1000).map(|i| format!("[main/INFO]: line {i}\n")).collect();
        let sent = Arc::new(Mutex::new(Vec::<Batch>::new()));
        let sink = sent.clone();
        let attached = attach(
            &console,
            "test",
            vec![(Box::new(std::io::Cursor::new(spam.into_bytes())), false), (Box::new(std::io::Cursor::new(b"oops\r\n".to_vec())), true)],
            None,
            move |b| sink.lock().unwrap().push(b),
        );
        attached.finish();
        let batches = sent.lock().unwrap();
        let delivered: usize = batches.iter().map(|b| b.lines.len()).sum();
        let skipped: u64 = batches.iter().map(|b| b.skipped).sum();
        assert_eq!(delivered as u64 + skipped, (CAPACITY + 1001) as u64);
        assert!(batches.iter().all(|b| b.lines.len() <= BATCH_MAX));
        let snapshot = console.snapshot("test");
        assert_eq!(snapshot.len(), CAPACITY);
        assert_eq!(snapshot.last().map(|l| l.n), Some((CAPACITY + 1001) as u64));

        // stderr without a level reads as a warning; CRLF is trimmed.
        let attached = attach(&console, "err", vec![(Box::new(std::io::Cursor::new(b"oops\r\n".to_vec())), true)], None, |_| {});
        attached.finish();
        let lines = console.snapshot("err");
        assert_eq!((lines[0].text.as_str(), lines[0].err, lines[0].level), ("oops", true, "warn"));
    }
}
