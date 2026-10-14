//! stdout/stderr -> logcat (tag `frogger`): the shared code logs with `eprintln!`.

use std::ffi::CString;
use std::io::{BufRead, BufReader};
use std::os::fd::FromRawFd;
use std::os::raw::{c_char, c_int};

#[link(name = "log")]
extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

const ANDROID_LOG_INFO: c_int = 4;

pub fn write(line: &str) {
    let tag = c"frogger";
    let text = CString::new(line.replace('\0', " ")).unwrap_or_default();
    unsafe { __android_log_write(ANDROID_LOG_INFO, tag.as_ptr(), text.as_ptr()) };
}

/// Points fds 1 and 2 at a pipe whose lines a thread forwards to logcat.
pub fn redirect_stdio() {
    let mut fds = [0 as c_int; 2];
    unsafe {
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            return;
        }
        libc::dup2(fds[1], 1);
        libc::dup2(fds[1], 2);
        libc::close(fds[1]);
    }
    let read = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    let _ = std::thread::Builder::new().name("logcat".into()).spawn(move || {
        for line in BufReader::new(read).lines() {
            match line {
                Ok(l) => write(&l),
                Err(_) => break,
            }
        }
    });
}
