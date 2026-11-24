use std::io;
use std::sync::Arc;

pub trait PagerRunner: Send + Sync {
    fn run_pager(&self, payload: &str) -> io::Result<()>;
}

impl<T: PagerRunner + ?Sized> PagerRunner for Arc<T> {
    fn run_pager(&self, payload: &str) -> io::Result<()> {
        (**self).run_pager(payload)
    }
}

impl<T: PagerRunner + ?Sized> PagerRunner for Box<T> {
    fn run_pager(&self, payload: &str) -> io::Result<()> {
        (**self).run_pager(payload)
    }
}

pub struct DefaultPagerRunner;

impl DefaultPagerRunner {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DefaultPagerRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl PagerRunner for DefaultPagerRunner {
    fn run_pager(&self, payload: &str) -> io::Result<()> {
        let pager_cmd = std::env::var("PAGER").unwrap_or_else(|_| "less -R".to_string());
        let mut parts = pager_cmd.split_whitespace();
        let program = parts.next().unwrap_or("less");
        let args: Vec<&str> = parts.collect();

        let mut child = std::process::Command::new(program)
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(payload.as_bytes());
        }

        let _ = child.wait()?;
        Ok(())
    }
}

pub type Pager = DefaultPagerRunner;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_card::DiffCard;
    use std::sync::Mutex;

    struct StubPagerRunner<F>
    where
        F: Fn(&str) -> io::Result<()> + Send + Sync,
    {
        handler: F,
    }

    impl<F> StubPagerRunner<F>
    where
        F: Fn(&str) -> io::Result<()> + Send + Sync,
    {
        fn new(handler: F) -> Self {
            Self { handler }
        }
    }

    impl<F> PagerRunner for StubPagerRunner<F>
    where
        F: Fn(&str) -> io::Result<()> + Send + Sync,
    {
        fn run_pager(&self, payload: &str) -> io::Result<()> {
            (self.handler)(payload)
        }
    }

    #[test]
    fn test_stub_pager_runner_works_with_diff_card() {
        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&captured);
        let diff = "--- a/f.rs\n+++ b/f.rs\n@@ -1 +1 @@\n-old\n+new\n";

        let card = DiffCard::builder()
            .diff_content(diff)
            .pager_runner(StubPagerRunner::new(move |payload: &str| {
                if let Ok(mut guard) = sink.lock() {
                    guard.push(payload.to_string());
                }
                Ok(())
            }))
            .build();

        assert_eq!(card.diff_content(), diff);

        let runner = StubPagerRunner::new(|payload: &str| {
            if payload.is_empty() {
                Err(io::Error::new(io::ErrorKind::InvalidInput, "empty payload"))
            } else {
                Ok(())
            }
        });
        assert!(runner.run_pager(card.diff_content()).is_ok());
        assert!(runner.run_pager("").is_err());

        let recording_sink = Arc::clone(&captured);
        let recording = StubPagerRunner::new(move |payload: &str| {
            if let Ok(mut guard) = recording_sink.lock() {
                guard.push(payload.to_string());
            }
            Ok(())
        });
        assert!(recording.run_pager(card.diff_content()).is_ok());
        let recorded = captured.lock().map(|g| g.clone()).unwrap_or_default();
        assert_eq!(recorded, vec![diff.to_string()]);
    }
}
