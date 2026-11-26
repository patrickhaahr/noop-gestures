use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, ensure};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::session::log;

type Connection = BufReader<UnixStream>;

pub struct Niri(mpsc::UnboundedSender<(&'static str, Instant)>);

impl Niri {
    #[must_use]
    pub fn connect_lazily() -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<(&'static str, Instant)>();
        tokio::spawn(async move {
            let mut connection = None;
            while let Some((action, dispatched)) = rx.recv().await {
                let Some(path) = std::env::var_os("NIRI_SOCKET") else {
                    log("cannot send niri action: NIRI_SOCKET is unset");
                    continue;
                };
                let request = send(&mut connection, Path::new(&path), action);
                match timeout(Duration::from_secs(1), request).await {
                    Ok(Ok(())) => log(&format!(
                        "niri {action} handled after {} ms",
                        dispatched.elapsed().as_millis()
                    )),
                    Ok(Err(e)) => log(&format!("niri {action}: {e:#}")),
                    Err(_) => log(&format!("niri {action}: reply timed out")),
                }
            }
        });
        Self(tx)
    }

    pub fn dispatch(&self, action: &'static str) {
        if self.0.send((action, Instant::now())).is_err() {
            log(&format!("niri {action}: the dispatcher has stopped"));
        }
    }
}

fn display(path: &Path) -> Option<&str> {
    let name = path
        .file_name()?
        .to_str()?
        .strip_prefix("niri.")?
        .strip_suffix(".sock")?;
    let (display, pid) = name.rsplit_once('.')?;
    pid.parse::<u32>().ok()?;
    Some(display)
}

async fn connect(path: &Path) -> io::Result<Connection> {
    let error = match UnixStream::connect(path).await {
        Ok(socket) => return Ok(BufReader::new(socket)),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            e
        }
        Err(e) => return Err(e),
    };
    let (Some(parent), Some(name)) = (path.parent(), display(path)) else {
        return Err(error);
    };
    let mut entries = tokio::fs::read_dir(parent).await?;
    while let Some(entry) = entries.next_entry().await? {
        let candidate = entry.path();
        if display(&candidate) == Some(name)
            && let Ok(socket) = UnixStream::connect(candidate).await
        {
            return Ok(BufReader::new(socket));
        }
    }
    Err(error)
}

async fn send(cached: &mut Option<Connection>, path: &Path, action: &str) -> Result<()> {
    let request = format!("{}\n", serde_json::json!({"Action": {action: {}}}));
    let mut connection = if let Some(mut conn) = cached.take()
        && conn.get_mut().write_all(request.as_bytes()).await.is_ok()
    {
        conn
    } else {
        let mut conn = connect(path)
            .await
            .with_context(|| format!("connecting to {}", path.display()))?;
        conn.get_mut().write_all(request.as_bytes()).await?;
        conn
    };
    let mut line = String::new();
    ensure!(
        connection.read_line(&mut line).await? > 0,
        "connection closed before reply"
    );
    let reply: Result<String, String> = serde_json::from_str(&line)?;
    *cached = Some(connection);
    let response = reply.map_err(|message| anyhow!(message))?;
    ensure!(response == "Handled", "unexpected response {response}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UnixListener;

    async fn receive(conn: &mut Connection, name: &str) -> Result<()> {
        let mut line = String::new();
        conn.read_line(&mut line).await?;
        assert_eq!(line, format!(r#"{{"Action":{{"{name}":{{}}}}}}"#) + "\n");
        Ok(())
    }

    #[tokio::test]
    async fn actions_share_a_connection_and_reconnect_after_a_pid_change() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("noop-niri-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("niri.wayland-1.10.sock");
        let replacement = dir.join("niri.wayland-1.11.sock");
        let listener = UnixListener::bind(&path)?;
        let mut cached = None;
        let server = async {
            let mut conn = BufReader::new(listener.accept().await?.0);
            for name in ["FocusColumnLeft", "FocusWorkspaceUp"] {
                receive(&mut conn, name).await?;
                conn.get_mut().write_all(b"{\"Ok\":\"Handled\"}\n").await?;
            }
            anyhow::Ok(())
        };
        let client = async {
            send(&mut cached, &path, "FocusColumnLeft").await?;
            send(&mut cached, &path, "FocusWorkspaceUp").await
        };
        timeout(Duration::from_secs(2), async {
            tokio::try_join!(server, client)
        })
        .await??;
        drop(listener);
        let listener = UnixListener::bind(&replacement)?;
        let server = async {
            let mut conn = BufReader::new(listener.accept().await?.0);
            receive(&mut conn, "ToggleOverview").await?;
            anyhow::Ok(())
        };
        let (received, result) = timeout(Duration::from_secs(2), async {
            tokio::join!(server, send(&mut cached, &path, "ToggleOverview"))
        })
        .await?;
        received?;
        assert!(result.is_err());
        assert!(cached.is_none());
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
