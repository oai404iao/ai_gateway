//! Child-process lifecycle helpers with log capture and graceful termination.

use std::{error::Error, fs::File, path::Path, process::Stdio, time::Duration};

use tokio::{
    process::{Child, Command},
    time::timeout,
};

pub struct ManagedChild {
    name: String,
    child: Child,
}

impl ManagedChild {
    pub fn spawn(
        name: impl Into<String>,
        command: &mut Command,
        log_path: &Path,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let stdout = File::create(log_path)?;
        let stderr = stdout.try_clone()?;
        command
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .kill_on_drop(true);
        let child = command.spawn()?;
        Ok(Self {
            name: name.into(),
            child,
        })
    }

    pub fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>, std::io::Error> {
        self.child.try_wait()
    }

    pub async fn wait(mut self) -> Result<std::process::ExitStatus, std::io::Error> {
        self.child.wait().await
    }

    pub async fn terminate(mut self, grace: Duration) -> Result<(), Box<dyn Error + Send + Sync>> {
        if self.child.try_wait()?.is_some() {
            return Ok(());
        }
        send_terminate(&self.child)?;
        match timeout(grace, self.child.wait()).await {
            Ok(result) => {
                result?;
            }
            Err(_) => {
                eprintln!(
                    "{} did not stop within {} seconds; killing it",
                    self.name,
                    grace.as_secs()
                );
                self.child.kill().await?;
                let _ = self.child.wait().await;
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
fn send_terminate(child: &Child) -> Result<(), std::io::Error> {
    let Some(id) = child.id() else {
        return Ok(());
    };
    // SAFETY: libc::kill does not retain the pointer-free integer arguments.
    let result = unsafe { libc::kill(id as libc::pid_t, libc::SIGTERM) };
    if result == 0 {
        Ok(())
    } else {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }
}

#[cfg(not(unix))]
fn send_terminate(_: &Child) -> Result<(), std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "graceful performance-process shutdown currently requires Unix",
    ))
}
