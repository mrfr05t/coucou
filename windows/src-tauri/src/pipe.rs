// The relay server coucou-hook talks to.
//
// Windows: named pipe `\\.\pipe\coucou-<sid>` — one instance per connection.
// Linux:   unix socket at `$XDG_RUNTIME_DIR/coucou.sock` — one accepted stream
//          per connection.
//
// Everything after `accept` is identical, and deliberately so: every hook event
// is forwarded to the island as a `hook` event, and `PermissionRequest` is the
// only one that keeps its connection open — it waits for the island's decision
// and writes it back on the same connection, which is how approving from the
// island works. The wire format lives in exactly one place, `handle`, because
// "the two platforms quietly drifted" is a bug nobody can reproduce.
//
// Claude Code is never blocked by us. Three things guarantee it:
//   * coucou-hook gives the connection 300 ms and exits cleanly if we are closed;
//   * we only wait for a human once the island has *confirmed* the card is on
//     screen, so a paused island or a webview that is not listening costs a few
//     hundred milliseconds, not two minutes;
//   * whatever happens we drop the connection after the decision timeout, and
//     the terminal takes over.
//
// What we write back is the bare word `allow` or `deny`. Turning that into the
// documented hookSpecificOutput JSON is coucou-hook's job, so the wire format
// Claude Code expects lives in exactly one place.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncRead, AsyncWrite, AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
#[cfg(windows)]
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::io::AsRawFd;

use crate::island::WINDOW_LABEL;
use crate::log;

/// Slightly under coucou-hook's own 110 s wait, so we always answer first.
const DECISION_TIMEOUT: Duration = Duration::from_secs(108);
/// How long the island gets to say "the card is up". This is the whole of B4:
/// without it, an island that is paused, hidden behind a crashed webview or
/// simply not listening would leave Claude Code staring at a prompt nobody can
/// see for nearly two minutes.
const ACK_TIMEOUT: Duration = Duration::from_millis(800);
const MAX_PAYLOAD: usize = 1 << 20;

/// What the island can say about a permission request.
pub enum Reply {
    /// The card is on screen and a human can act on it.
    Ack,
    /// A human clicked: `allow` or `deny`.
    Decision(String),
    /// Nobody can act on it — paused, or another request already holds the card.
    Decline,
}

/// Permission requests the island has been told about.
#[derive(Default)]
pub struct Pending(pub Mutex<HashMap<String, mpsc::Sender<Reply>>>);

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// A connected peer, whichever transport it arrived on.
///
/// `AsyncRead + AsyncWrite` is the whole wire protocol and is the same on both
/// platforms by design; `hang_up` is the one operation that is not, so it is
/// the one thing this trait adds. Its default is right for a unix socket —
/// EOF is what closing does — and Windows overrides it to disconnect in the
/// orderly way a named-pipe client is waiting for.
trait Peer: AsyncRead + AsyncWrite + Unpin + Send + 'static {
    fn hang_up(&mut self) {}
}

#[cfg(windows)]
impl Peer for NamedPipeServer {
    fn hang_up(&mut self) {
        let _ = NamedPipeServer::disconnect(self);
    }
}

#[cfg(unix)]
impl Peer for UnixStream {}

/// Where coucou-hook must connect. **Must match the relay's `pipe_path()`
/// exactly** — the two are separate binaries and nothing checks this for them.
#[cfg(windows)]
pub fn endpoint() -> String {
    let key = crate::platform::current_user_key()
        .unwrap_or_else(|| std::env::var("USERNAME").unwrap_or_else(|_| "user".into()));
    format!(r"\\.\pipe\coucou-{key}")
}

/// `$XDG_RUNTIME_DIR/coucou.sock`, falling back to `/tmp/coucou-<uid>.sock`.
///
/// `$XDG_RUNTIME_DIR` is created mode 0700 for this uid by the OS, so the
/// socket inside it is unreachable to anyone else by construction; the file
/// mode below is belt and braces. The `/tmp` fallback covers a session that
/// never set the variable and carries the uid in the name for the same reason.
#[cfg(unix)]
pub fn endpoint() -> String {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return std::path::PathBuf::from(dir)
            .join("coucou.sock")
            .to_string_lossy()
            .into_owned();
    }
    let uid = crate::platform::current_user_key().unwrap_or_else(|| "0".into());
    format!("/tmp/coucou-{uid}.sock")
}

#[cfg(windows)]
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let name = endpoint();
        // first_pipe_instance also means we refuse to join a pipe somebody else
        // already owns under our name, rather than serving on top of it.
        let mut server = match ServerOptions::new().first_pipe_instance(true).create(&name) {
            Ok(s) => s,
            Err(err) => {
                log::line(format!("cannot open the relay pipe: {err}"));
                return;
            }
        };
        loop {
            if server.connect().await.is_err() {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
            // Hand the connected instance to a task and listen on a fresh one.
            let next = match ServerOptions::new().create(&name) {
                Ok(s) => s,
                Err(err) => {
                    log::line(format!("cannot reopen the relay pipe: {err}"));
                    return;
                }
            };
            let connected = std::mem::replace(&mut server, next);
            let app = app.clone();
            tauri::async_runtime::spawn(async move { handle(app, connected).await });
        }
    });
}

#[cfg(unix)]
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let path = endpoint();
        let listener = match bind(&path) {
            Ok(l) => l,
            Err(err) => {
                log::line(format!("cannot open the relay socket at {path}: {err}"));
                return;
            }
        };
        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    if !peer_is_same_user(&stream) {
                        log::line("relay socket: refused a peer running as another user");
                        continue;
                    }
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move { handle(app, stream).await });
                }
                Err(err) => {
                    // ENFILE/EMFILE: the process is out of descriptors. Wailing
                    // would spin at 100%, so back off and keep serving.
                    log::line(format!("relay socket accept failed: {err}"));
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    });
}

/// Binds the socket, dealing with the one that the last run left behind.
///
/// A unix socket file is not removed when its process dies, so after a crash
/// `bind` returns `EADDRINUSE` forever and the relay never comes back — the
/// classic "works until you kill it once" bug. The fix is to notice that the
/// file is ours (a socket), that nobody answers it (the listener is gone) and
/// only then unlink it. A file that *is* being answered means another live
/// instance of Coucou is running, and taking its socket from under it would be
/// worse than not starting.
#[cfg(unix)]
fn bind(path: &str) -> std::io::Result<UnixListener> {
    use std::os::unix::fs::FileTypeExt;

    let err = match UnixListener::bind(path) {
        Ok(listener) => {
            // 0600: readable and writable by us only. `$XDG_RUNTIME_DIR` already
            // stops anyone else reaching the file; this stops a future change to
            // that from quietly becoming an open relay.
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            return Ok(listener);
        }
        Err(err) => err,
    };

    // Only an address conflict is ours to investigate.
    if err.raw_os_error() != Some(libc::EADDRINUSE) {
        return Err(err);
    }
    let Ok(meta) = std::fs::metadata(path) else { return Err(err) };
    if !meta.file_type().is_socket() {
        return Err(err); // Something else owns that name. Never unlink it.
    }
    // `EADDRINUSE` with a live listener means another instance of Coucou is
    // running, and taking its socket from under it would be worse than not
    // starting. A listener that is gone answers `ECONNREFUSED`.
    if std::os::unix::net::UnixStream::connect(path).is_ok() {
        return Err(err);
    }

    log::line(format!("removing a stale relay socket at {path}"));
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    Ok(listener)
}

/// `SO_PEERCRED`: the uid of whoever just connected.
///
/// Treated the same way Windows treats "I cannot read the SID": a peer we
/// cannot vouch for is refused. The socket's own mode already makes this
/// unreachable, but it is the mirror image of the client's check and costs one
/// getsockopt.
#[cfg(unix)]
#[derive(Default)]
#[repr(C)]
struct UCred {
    pid: i32,
    uid: u32,
    gid: u32,
}

#[cfg(unix)]
fn peer_is_same_user<S: AsRawFd>(stream: &S) -> bool {
    let mine = std::fs::metadata("/proc/self")
        .ok()
        .map(|m| {
            use std::os::unix::fs::MetadataExt;
            m.uid()
        });
    let Some(mine) = mine else { return false };

    let mut cred = UCred::default();
    let mut len = std::mem::size_of::<UCred>() as u32;
    // SAFETY: `cred` is a valid buffer of exactly `len` bytes for SO_PEERCRED,
    // and the fd comes from a stream the kernel just accepted for us.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::from_mut(&mut cred).cast(),
            std::ptr::from_mut(&mut len),
        )
    };
    rc == 0 && cred.uid == mine
}

async fn handle<C: Peer>(app: AppHandle, mut peer: C) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match peer.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.contains(&b'\n') || buf.len() > MAX_PAYLOAD {
                    break;
                }
            }
            Err(_) => return,
        }
    }
    let line = match buf.iter().position(|b| *b == b'\n') {
        Some(i) => &buf[..i],
        None => &buf[..],
    };
    let Ok(mut payload) = serde_json::from_slice::<Value>(line) else { return };
    if !payload.is_object() {
        return;
    }

    let event = payload
        .get("hook_event_name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    if event != "PermissionRequest" {
        log::line(format!("hook {event}"));
        let _ = app.emit_to(WINDOW_LABEL, "hook", payload);
        peer.hang_up();
        return;
    }

    let id = format!("{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed));
    let (tx, mut rx) = mpsc::channel::<Reply>(4);
    {
        let pending = app.state::<Pending>();
        pending.0.lock().unwrap().insert(id.clone(), tx);
    }
    payload["request_id"] = json!(id);
    log::line(format!("hook PermissionRequest id={id}"));
    let _ = app.emit_to(WINDOW_LABEL, "hook", payload);

    let decision = wait_for_decision(&id, &mut rx).await;
    app.state::<Pending>().0.lock().unwrap().remove(&id);

    // No decision: say nothing at all. coucou-hook then writes nothing to stdout
    // and Claude Code asks in the terminal, exactly as if Coucou were closed.
    if let Some(d) = decision {
        let _ = peer.write_all(format!("{d}\n").as_bytes()).await;
        let _ = peer.flush().await;
    }
    peer.hang_up();
}

/// Two waits: a short one for "the card is up", then the long one for a human.
async fn wait_for_decision(id: &str, rx: &mut mpsc::Receiver<Reply>) -> Option<String> {
    match tokio::time::timeout(ACK_TIMEOUT, rx.recv()).await {
        Ok(Some(Reply::Ack)) => {}
        // A click that beats the ack is still a click.
        Ok(Some(Reply::Decision(d))) => {
            log::line(format!("hook id={id} answered {d}"));
            return Some(d);
        }
        Ok(Some(Reply::Decline)) => {
            log::line(format!("hook id={id} not shown — terminal takes over"));
            return None;
        }
        Ok(None) => return None,
        Err(_) => {
            log::line(format!("hook id={id} island never acknowledged — terminal takes over"));
            return None;
        }
    }

    match tokio::time::timeout(DECISION_TIMEOUT, rx.recv()).await {
        Ok(Some(Reply::Decision(d))) => {
            log::line(format!("hook id={id} answered {d}"));
            Some(d)
        }
        Ok(Some(Reply::Decline)) => {
            log::line(format!("hook id={id} released without a decision"));
            None
        }
        _ => {
            log::line(format!("hook id={id} timed out — terminal takes over"));
            None
        }
    }
}

fn send(app: &AppHandle, request_id: &str, reply: Reply, keep: bool) {
    let sender = {
        let pending = app.state::<Pending>();
        let mut map = pending.0.lock().unwrap();
        if keep { map.get(request_id).cloned() } else { map.remove(request_id) }
    };
    match sender {
        Some(tx) => {
            let _ = tx.try_send(reply);
        }
        None => log::line(format!("reply for id={request_id} — no pending request")),
    }
}

/// The island has the card on screen; the long wait may begin.
pub fn acknowledge(app: &AppHandle, request_id: &str) {
    send(app, request_id, Reply::Ack, true);
}

/// Nobody can act on this one — paused, or another card already holds the view.
pub fn decline(app: &AppHandle, request_id: &str) {
    log::line(format!("decline id={request_id}"));
    send(app, request_id, Reply::Decline, false);
}

/// Called by the island's Allow / Deny buttons. Only ever a bare word: turning
/// it into Claude Code's JSON is coucou-hook's job.
pub fn answer(app: &AppHandle, request_id: &str, decision: &str) {
    let word = match decision {
        "allow" | "always" => "allow",
        _ => "deny",
    };
    log::line(format!("decision id={request_id} {word}"));
    send(app, request_id, Reply::Decision(word.to_string()), false);
}
