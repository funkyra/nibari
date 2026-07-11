use std::io::Write;

use log::{LevelFilter, Log, Metadata, Record};

static LOGGER: StderrLogger = StderrLogger;

pub fn init() {
    let level = std::env::var("RUST_LOG")
        .ok()
        .as_deref()
        .and_then(parse_level)
        .unwrap_or(LevelFilter::Info);

    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(level);
    }
}

struct StderrLogger;

impl Log for StderrLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            let stderr = std::io::stderr();
            let mut stderr = stderr.lock();
            let _ = writeln!(stderr, "[{}] {}", record.level(), record.args());
        }
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

fn parse_level(directives: &str) -> Option<LevelFilter> {
    let (global, nibari) =
        directives
            .split(',')
            .map(str::trim)
            .fold(
                (None, None),
                |(global, nibari), directive| match directive.split_once('=') {
                    Some(("nibari", level)) => (global, level.parse().ok().or(nibari)),
                    Some(_) => (global, nibari),
                    None if !directive.is_empty() => (directive.parse().ok().or(global), nibari),
                    None => (global, nibari),
                },
            );
    nibari.or(global)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_global_and_target_levels() {
        assert_eq!(parse_level("debug"), Some(LevelFilter::Debug));
        assert_eq!(parse_level("warn,nibari=trace"), Some(LevelFilter::Trace));
        assert_eq!(parse_level("other=debug"), None);
    }
}
