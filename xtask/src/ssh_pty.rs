use std::io;
use std::time::{Duration, Instant};

pub fn cli(args: &[String]) -> i32 {
    if args.len() != 2 {
        eprintln!("usage: xtask ssh-pty CONFIG HOST");
        return 2;
    }
    match run(&args[0], &args[1]) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn run(config: &str, host: &str) -> Result<(), String> {
    let mut master_fd: libc::c_int = -1;
    let pid = unsafe {
        libc::forkpty(
            &mut master_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if pid < 0 {
        return Err(format!("forkpty failed: {}", io::Error::last_os_error()));
    }
    if pid == 0 {
        let ssh = c_string("ssh");
        let tt = c_string("-tt");
        let flag = c_string("-F");
        let config = c_string(config);
        let host = c_string(host);
        let argv = [
            ssh.as_ptr(),
            tt.as_ptr(),
            flag.as_ptr(),
            config.as_ptr(),
            host.as_ptr(),
            std::ptr::null(),
        ];
        unsafe { libc::execvp(ssh.as_ptr(), argv.as_ptr()) };
        unsafe { libc::_exit(127) };
    }
    let master = Master(master_fd);
    resize(master.0, pid, 31, 97)?;
    write_all(master.0, b"printf '__SIZE__'; stty size\n")?;
    let mut output = Vec::new();
    expect(master.0, &mut output, b"__SIZE__31 97")?;
    resize(master.0, pid, 43, 111)?;
    write_all(master.0, b"printf '__RESIZE__'; stty size\n")?;
    expect(master.0, &mut output, b"__RESIZE__43 111")?;
    write_all(master.0, b"echo __SLEEPING__; sleep 30\n")?;
    expect(master.0, &mut output, b"\r\n__SLEEPING__\r\n")?;
    write_all(master.0, &[0x03])?;
    write_all(master.0, b"printf '\\n__INTERRUPTED__\\n'\n")?;
    expect(master.0, &mut output, b"\r\n__INTERRUPTED__\r\n")?;
    write_all(master.0, b"exit\n")?;
    let mut status = 0;
    let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
    if waited < 0 {
        return Err("waitpid failed".into());
    }
    if !libc::WIFEXITED(status) || libc::WEXITSTATUS(status) != 0 {
        return Err(format!("ssh exited with status {status}"));
    }
    println!("PASS: interactive PTY, terminal resize and Ctrl-C");
    Ok(())
}

struct Master(libc::c_int);

impl Drop for Master {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

fn resize(master: libc::c_int, pid: libc::pid_t, rows: u16, columns: u16) -> Result<(), String> {
    let size = libc::winsize {
        ws_row: rows,
        ws_col: columns,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let result = unsafe { libc::ioctl(master, libc::TIOCSWINSZ, &size) };
    if result < 0 {
        return Err(format!("ioctl failed: {}", io::Error::last_os_error()));
    }
    if unsafe { libc::kill(pid, libc::SIGWINCH) } < 0 {
        return Err(format!("kill failed: {}", io::Error::last_os_error()));
    }
    Ok(())
}

fn write_all(master: libc::c_int, bytes: &[u8]) -> Result<(), String> {
    let mut offset = 0;
    while offset < bytes.len() {
        let wrote = unsafe {
            libc::write(
                master,
                bytes[offset..].as_ptr().cast(),
                bytes.len() - offset,
            )
        };
        if wrote < 0 {
            return Err(format!("pty write failed: {}", io::Error::last_os_error()));
        }
        offset += wrote as usize;
    }
    Ok(())
}

fn expect(master: libc::c_int, output: &mut Vec<u8>, value: &[u8]) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !contains(output, value) {
        if Instant::now() > deadline {
            let start = output.len().saturating_sub(3000);
            return Err(format!("missing {value:?}: {:?}", &output[start..]));
        }
        let mut read_set = unsafe { std::mem::zeroed::<libc::fd_set>() };
        unsafe {
            libc::FD_ZERO(&mut read_set);
            libc::FD_SET(master, &mut read_set);
        }
        let mut timeout = libc::timeval {
            tv_sec: 0,
            tv_usec: 100_000,
        };
        let ready = unsafe {
            libc::select(
                master + 1,
                &mut read_set,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut timeout,
            )
        };
        if ready > 0 {
            let mut buffer = [0u8; 8192];
            let count = unsafe { libc::read(master, buffer.as_mut_ptr().cast(), buffer.len()) };
            if count > 0 {
                output.extend_from_slice(&buffer[..count as usize]);
                if contains(output, b"System is booting up") {
                    return Err("ready guest still reports pam_nologin boot restriction".into());
                }
            }
        }
    }
    output.clear();
    Ok(())
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn c_string(value: &str) -> std::ffi::CString {
    std::ffi::CString::new(value).unwrap_or_else(|_| std::ffi::CString::new("").unwrap())
}
