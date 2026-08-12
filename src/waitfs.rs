//! Block until a directory's contents change: kqueue on the BSDs and
//! macOS, inotify on Linux. The kernel is the notification system; this
//! module only holds the subscription.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::Duration;

pub struct DirWatch {
    queue: i32,
    // Held open: the kqueue subscription is on this descriptor.
    #[allow(dead_code)]
    dir: Option<File>,
}

impl Drop for DirWatch {
    fn drop(&mut self) {
        unsafe { libc::close(self.queue) };
    }
}

#[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd"))]
impl DirWatch {
    pub fn new(dir: &Path) -> io::Result<DirWatch> {
        let dir = File::open(dir)?;
        let queue = unsafe { libc::kqueue() };
        if queue < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut ev: libc::kevent = unsafe { std::mem::zeroed() };
        ev.ident = dir.as_raw_fd() as usize;
        ev.filter = libc::EVFILT_VNODE;
        ev.flags = libc::EV_ADD | libc::EV_CLEAR;
        ev.fflags = libc::NOTE_WRITE | libc::NOTE_EXTEND;
        let r = unsafe {
            libc::kevent(queue, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null())
        };
        if r < 0 {
            let e = io::Error::last_os_error();
            unsafe { libc::close(queue) };
            return Err(e);
        }
        Ok(DirWatch { queue, dir: Some(dir) })
    }

    /// True when the directory changed, false on timeout. A signal
    /// surfaces as ErrorKind::Interrupted: the caller owns the deadline
    /// and must recompute the remaining time before retrying.
    pub fn wait(&self, timeout: Option<Duration>) -> io::Result<bool> {
        let ts = timeout.map(|d| libc::timespec {
            tv_sec: d.as_secs() as libc::time_t,
            tv_nsec: d.subsec_nanos() as libc::c_long,
        });
        let mut out: libc::kevent = unsafe { std::mem::zeroed() };
        let r = unsafe {
            libc::kevent(
                self.queue,
                std::ptr::null(),
                0,
                &mut out,
                1,
                ts.as_ref().map_or(std::ptr::null(), |t| t),
            )
        };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(r > 0)
    }
}

#[cfg(target_os = "linux")]
impl DirWatch {
    pub fn new(dir: &Path) -> io::Result<DirWatch> {
        use std::os::unix::ffi::OsStrExt;
        let queue = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
        if queue < 0 {
            return Err(io::Error::last_os_error());
        }
        let c = std::ffi::CString::new(dir.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path has NUL"))?;
        let w = unsafe {
            libc::inotify_add_watch(queue, c.as_ptr(), libc::IN_CREATE | libc::IN_MOVED_TO)
        };
        if w < 0 {
            let e = io::Error::last_os_error();
            unsafe { libc::close(queue) };
            return Err(e);
        }
        Ok(DirWatch { queue, dir: None })
    }

    /// True when the directory changed, false on timeout. A signal
    /// surfaces as ErrorKind::Interrupted: the caller owns the deadline
    /// and must recompute the remaining time before retrying.
    pub fn wait(&self, timeout: Option<Duration>) -> io::Result<bool> {
        let mut pfd = libc::pollfd {
            fd: self.queue,
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = timeout.map_or(-1i32, |d| d.as_millis().min(i32::MAX as u128) as i32);
        let r = unsafe { libc::poll(&mut pfd, 1, ms) };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        if r == 0 {
            return Ok(false);
        }
        // Drain the event buffer; the caller rescans the directory, so
        // the contents don't matter.
        let mut buf = [0u8; 4096];
        unsafe { libc::read(self.queue, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        Ok(true)
    }
}
