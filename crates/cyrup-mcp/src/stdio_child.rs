//! The stdio server's child process and transport — rmcp's `TokioChildProcess`
//! (`transport/child_process.rs` @3.1.4), with the child's stdout read through
//! [`crate::depth_guard::DepthGuard`].
//!
//! rmcp builds its `AsyncRwTransport` over the raw `ChildStdout` inside `spawn`, so nothing between
//! the pipe and its line codec can be reached from outside; owning the child is the only way to
//! measure a line before the codec drops it (MCP-630). Everything else is rmcp's, line for line: the
//! three pipes, the stderr pipe only when asked for, `graceful_shutdown`'s close-then-wait with the
//! kill after [`MAX_WAIT_ON_DROP`], and a drop that kills a child nobody closed.

use std::process::Stdio;
use std::time::Duration;

use rmcp::RoleClient;
use rmcp::transport::Transport;
use rmcp::transport::async_rw::AsyncRwTransport;
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};

use crate::depth_guard::DepthGuard;

/// `MAX_WAIT_ON_DROP_SECS` (`child_process.rs:10`): how long a closed child has to exit before it
/// is killed.
pub const MAX_WAIT_ON_DROP: Duration = Duration::from_secs(3);

type GuardedTransport = AsyncRwTransport<RoleClient, DepthGuard<ChildStdout>, ChildStdin>;

/// A spawned stdio server and the transport over its pipes.
pub struct StdioChild {
    child: KillOnDrop,
    transport: GuardedTransport,
}

/// `ChildWithCleanup` (`child_process.rs:41-57`): a child nobody closed is killed — `kill`, not
/// `start_kill`, so it is also reaped.
struct KillOnDrop(Option<Child>);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let Some(mut child) = self.0.take() else {
            return;
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    if let Err(error) = child.kill().await {
                        tracing::warn!("Error killing child process: {error}");
                    }
                });
            }
            // rmcp's `tokio::spawn` panics here; without a runtime the child can still be signalled.
            Err(_) => {
                if let Err(error) = child.start_kill() {
                    tracing::warn!("Error killing child process: {error}");
                }
            }
        }
    }
}

impl StdioChild {
    /// `TokioChildProcessBuilder::spawn` (`child_process.rs:150-165`): stdin and stdout piped,
    /// stderr piped when `pipe_stderr` and inherited otherwise, handed back when piped.
    ///
    /// # Errors
    ///
    /// The spawn's own error, or rmcp's `"stdin was already taken"` / `"stdout was already taken"`.
    pub fn spawn(
        mut command: tokio::process::Command,
        pipe_stderr: bool,
    ) -> std::io::Result<(Self, Option<ChildStderr>)> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(if pipe_stderr {
                Stdio::piped()
            } else {
                Stdio::inherit()
            });
        let mut child = command.spawn()?;
        let Some(stdin) = child.stdin.take() else {
            return Err(std::io::Error::other("stdin was already taken"));
        };
        let Some(stdout) = child.stdout.take() else {
            return Err(std::io::Error::other("stdout was already taken"));
        };
        let stderr = child.stderr.take();
        Ok((
            Self {
                child: KillOnDrop(Some(child)),
                transport: AsyncRwTransport::new(DepthGuard::new(stdout), stdin),
            },
            stderr,
        ))
    }

    /// The child's process id, until it is closed.
    #[must_use]
    pub fn id(&self) -> Option<u32> {
        self.child.0.as_ref()?.id()
    }

    /// `graceful_shutdown` (`child_process.rs:81-107`): close the transport, which closes the
    /// child's stdin, then wait [`MAX_WAIT_ON_DROP`] for it to exit before killing it.
    ///
    /// # Errors
    ///
    /// The kill's or the wait's error.
    pub async fn graceful_shutdown(&mut self) -> std::io::Result<()> {
        let Some(mut child) = self.child.0.take() else {
            return Ok(());
        };
        self.transport.close().await?;
        tokio::select! {
            () = tokio::time::sleep(MAX_WAIT_ON_DROP) => {
                if let Err(error) = child.kill().await {
                    tracing::warn!("Error killing child: {error}");
                    return Err(error);
                }
            }
            status = child.wait() => match status {
                Ok(status) => tracing::info!("Child exited gracefully {status}"),
                Err(error) => {
                    tracing::warn!("Error waiting for child: {error}");
                    return Err(error);
                }
            },
        }
        Ok(())
    }
}

impl Transport<RoleClient> for StdioChild {
    type Error = std::io::Error;

    fn send(
        &mut self,
        item: rmcp::service::TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.transport.send(item)
    }

    fn receive(
        &mut self,
    ) -> impl Future<Output = Option<rmcp::service::RxJsonRpcMessage<RoleClient>>> + Send {
        self.transport.receive()
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.graceful_shutdown()
    }
}
