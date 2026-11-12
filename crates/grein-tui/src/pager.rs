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

pub struct MockPagerRunner<F>
where
    F: Fn(&str) -> io::Result<()> + Send + Sync,
{
    handler: F,
}

impl<F> MockPagerRunner<F>
where
    F: Fn(&str) -> io::Result<()> + Send + Sync,
{
    pub fn new(handler: F) -> Self {
        Self { handler }
    }
}

impl<F> PagerRunner for MockPagerRunner<F>
where
    F: Fn(&str) -> io::Result<()> + Send + Sync,
{
    fn run_pager(&self, payload: &str) -> io::Result<()> {
        (self.handler)(payload)
    }
}
