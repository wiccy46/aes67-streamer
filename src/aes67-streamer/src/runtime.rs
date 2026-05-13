use anyhow::Result;
use tokio_util::sync::CancellationToken;

use crate::streamer::Aes67Streamer;

pub struct RuntimeSupervisor {
    shutdown_token: CancellationToken,
}

impl RuntimeSupervisor {
    pub fn new() -> Self {
        Self {
            shutdown_token: CancellationToken::new(),
        }
    }

    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown_token.clone()
    }

    #[cfg(test)]
    pub fn request_shutdown(&self) {
        self.shutdown_token.cancel();
    }

    pub async fn run_streamer(&self, streamer: &mut Aes67Streamer) -> Result<()> {
        let signal_token = self.shutdown_token();
        let stream_token = self.shutdown_token();
        let signal_task = tokio::spawn(async move {
            wait_for_os_shutdown().await;
            signal_token.cancel();
        });

        let result = streamer.run_until_cancelled(stream_token).await;
        signal_task.abort();
        if let Err(e) = signal_task.await {
            if !e.is_cancelled() {
                log::warn!("Shutdown signal task failed to join: {e}");
            }
        }
        result
    }
}

impl Default for RuntimeSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

async fn wait_for_os_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let interrupt = tokio::signal::ctrl_c();
        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(e) => {
                log::warn!("Failed to install SIGTERM handler: {e}");
                if let Err(e) = interrupt.await {
                    log::warn!("Failed to listen for Ctrl-C: {e}");
                }
                return;
            }
        };

        tokio::select! {
            result = interrupt => {
                if let Err(e) = result {
                    log::warn!("Failed to listen for Ctrl-C: {e}");
                } else {
                    log::info!("Received Ctrl-C");
                }
            }
            _ = terminate.recv() => {
                log::info!("Received SIGTERM");
            }
        }
    }

    #[cfg(not(unix))]
    {
        if let Err(e) = tokio::signal::ctrl_c().await {
            log::warn!("Failed to listen for Ctrl-C: {e}");
        } else {
            log::info!("Received Ctrl-C");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn supervisor_manual_shutdown_cancels_child_token() {
        let supervisor = RuntimeSupervisor::new();
        let token = supervisor.shutdown_token();

        assert!(!token.is_cancelled());
        supervisor.request_shutdown();

        token.cancelled().await;
        assert!(token.is_cancelled());
    }
}
