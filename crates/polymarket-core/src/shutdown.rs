//! Stop signals for the service binaries.
//!
//! Each binary runs as PID 1 in its container, where SIGTERM has no default
//! action: a service that does not listen for it is killed only when the
//! stop grace period runs out.

/// SIGTERM (container stop, deploy) and Ctrl-C, listened for from
/// [`StopSignals::install`] on, so a signal that arrives before anything
/// waits for it is not lost.
pub struct StopSignals {
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
}

impl StopSignals {
    pub fn install() -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            Ok(Self {
                terminate: signal(SignalKind::terminate())?,
                interrupt: signal(SignalKind::interrupt())?,
            })
        }
        #[cfg(not(unix))]
        Ok(Self {})
    }

    /// Resolve on the next stop signal.
    pub async fn recv(&mut self) {
        #[cfg(unix)]
        tokio::select! {
            _ = self.terminate.recv() => {}
            _ = self.interrupt.recv() => {}
        }
        #[cfg(not(unix))]
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_signal_sent_before_anyone_waits_is_kept() {
        let mut stop = StopSignals::install().unwrap();
        let sent = std::process::Command::new("kill")
            .args(["-TERM", &std::process::id().to_string()])
            .status()
            .unwrap();
        assert!(sent.success());
        tokio::time::timeout(std::time::Duration::from_secs(5), stop.recv())
            .await
            .expect("the earlier SIGTERM is delivered");
    }
}
