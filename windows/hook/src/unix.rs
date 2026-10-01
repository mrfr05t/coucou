//! The little bit of Unix the relay needs: who we are, and who is on the other
//! end of the socket.
//!
//! Unlike a named pipe, a socket at `$XDG_RUNTIME_DIR/coucou.sock` sits in a
//! directory the OS already created mode 0700 for this uid, so a stranger
//! cannot get there first. `SO_PEERCRED` is still checked, because "cannot
//! happen" is the sentence that precedes every incident: a socket file that
//! somehow exists with the wrong ownership gets nothing from us, and a failure
//! to answer is read as "not ours".

use std::os::unix::io::AsRawFd;

/// `ucred` as the kernel lays it out on Linux: pid, uid, gid.
#[repr(C)]
#[derive(Default)]
struct UCred {
    pid: i32,
    uid: u32,
    gid: u32,
}

/// True when the process at the other end of `stream` runs as the same user we
/// do.
///
/// A failure to answer is treated as "not ours": refusing to talk to a socket
/// we cannot vouch for costs one hook event, while trusting it could hand
/// another account the contents of every tool call.
pub fn peer_is_same_user<S: AsRawFd>(stream: &S) -> bool {
    let Some(mine) = current_uid() else { return false };
    let mut cred = UCred::default();
    let mut len = std::mem::size_of::<UCred>() as u32;
    // SAFETY: `cred` is a valid, correctly sized buffer for `SO_PEERCRED`, and
    // `len` is in/out exactly as getsockopt expects. The fd comes from a live
    // socket we just connected.
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

/// The uid this process runs as. `/proc/self` is owned by the process it
/// names, so its uid is our euid.
pub fn current_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").ok().map(|m| m.uid())
}
