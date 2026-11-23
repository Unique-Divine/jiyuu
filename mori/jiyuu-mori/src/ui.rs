//! Foreground command for the loopback graph UI.

use std::ffi::{c_char, c_int, c_void};
use std::io::{self, Write};
use std::mem::MaybeUninit;
use std::ptr::{self, NonNull};
use std::thread;

const DEFAULT_PORT: u16 = 9749;
const SHUTDOWN_GRACE_SECONDS: libc::c_uint = 5;

unsafe extern "C" {
    fn cbm_alloc_init();
    fn cbm_log_init_from_env();
    fn cbm_embedded_lookup(path: *const c_char) -> *const c_void;
    fn cbm_http_server_new_read_only(port: c_int) -> *mut c_void;
    fn cbm_http_server_free(server: *mut c_void) -> bool;
    fn cbm_http_server_schedule_run(server: *mut c_void) -> bool;
    fn cbm_http_server_cancel_scheduled_run(server: *mut c_void) -> bool;
    fn cbm_http_server_run(server: *mut c_void);
    fn cbm_http_server_stop(server: *mut c_void);
}

struct Server(NonNull<c_void>);

// SAFETY: only the scoped worker runs the event loop. Its only concurrent
// caller is stop(), which the C API permits. Drop runs after the worker joins.
unsafe impl Sync for Server {}

impl Server {
    fn bind(port: u16) -> Result<Self, String> {
        // SAFETY: the allocator has been initialized and no HTTP loop is running.
        NonNull::new(unsafe { cbm_http_server_new_read_only(port.into()) })
            .map(Self)
            .ok_or_else(|| format!("could not start the UI on 127.0.0.1:{port}"))
    }

    fn serve(&self, port: u16, signals: &libc::sigset_t) -> Result<(), String> {
        let server = self.0.as_ptr();
        // SAFETY: the freshly bound server has no running or scheduled worker.
        if !unsafe { cbm_http_server_schedule_run(server) } {
            return Err("could not schedule the HTTP loop".into());
        }
        thread::scope(|scope| {
            if let Err(error) = thread::Builder::new()
                .name("mori-ui".into())
                .spawn_scoped(scope, || self.run())
            {
                // SAFETY: the worker failed to start, so this is still scheduled.
                unsafe { cbm_http_server_cancel_scheduled_run(server) };
                return Err(format!("could not start the HTTP loop: {error}"));
            }

            let _ = writeln!(
                io::stderr(),
                "Mori UI listening at http://127.0.0.1:{port}"
            );
            let mut signal = 0;
            // SAFETY: the signal set is initialized and blocked in both threads.
            let status = unsafe { libc::sigwait(signals, &mut signal) };
            // SAFETY: the worker still borrows the live server. A process
            // alarm bounds cancellation, join and cleanup even if a native
            // reader ignores cancellation. The process exits after this command.
            unsafe {
                libc::alarm(SHUTDOWN_GRACE_SECONDS);
                cbm_http_server_stop(server);
            }
            signal_result(status)
                .map_err(|error| format!("could not wait for shutdown: {error}"))
        })
    }

    fn run(&self) {
        // SAFETY: serve schedules exactly one event loop and keeps self alive.
        unsafe { cbm_http_server_run(self.0.as_ptr()) };
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // SAFETY: the worker has joined, or its schedule was cancelled.
        if !unsafe { cbm_http_server_free(self.0.as_ptr()) } {
            std::process::abort();
        }
    }
}

fn signal_result(status: c_int) -> io::Result<()> {
    match status {
        0 => Ok(()),
        error => Err(io::Error::from_raw_os_error(error)),
    }
}

fn block_shutdown_signals() -> io::Result<libc::sigset_t> {
    let mut signals = MaybeUninit::zeroed();
    // SAFETY: sigemptyset initializes the set, and sigaddset takes valid signals.
    unsafe {
        if libc::sigemptyset(signals.as_mut_ptr()) != 0
            || libc::sigaddset(signals.as_mut_ptr(), libc::SIGINT) != 0
            || libc::sigaddset(signals.as_mut_ptr(), libc::SIGTERM) != 0
        {
            return Err(io::Error::last_os_error());
        }
        let signals = signals.assume_init();
        // The worker inherits this mask. The command exits the process when done.
        signal_result(libc::pthread_sigmask(
            libc::SIG_BLOCK,
            &signals,
            ptr::null_mut(),
        ))?;
        // Background jobs can inherit SIGINT ignored. Restore the default
        // dispositions with both signals blocked so sigwait can consume them.
        if libc::signal(libc::SIGINT, libc::SIG_DFL) == libc::SIG_ERR
            || libc::signal(libc::SIGTERM, libc::SIG_DFL) == libc::SIG_ERR
            || libc::signal(libc::SIGALRM, libc::SIG_DFL) == libc::SIG_ERR
        {
            return Err(io::Error::last_os_error());
        }
        let mut alarm = MaybeUninit::zeroed();
        if libc::sigemptyset(alarm.as_mut_ptr()) != 0
            || libc::sigaddset(alarm.as_mut_ptr(), libc::SIGALRM) != 0
        {
            return Err(io::Error::last_os_error());
        }
        signal_result(libc::pthread_sigmask(
            libc::SIG_UNBLOCK,
            alarm.as_ptr(),
            ptr::null_mut(),
        ))?;
        Ok(signals)
    }
}

fn start(port: u16) -> Result<(), String> {
    let signals = block_shutdown_signals()
        .map_err(|error| format!("could not block shutdown signals: {error}"))?;
    // SAFETY: the asset lookup takes a static C string and returns a borrowed entry.
    if unsafe { cbm_embedded_lookup(c"/index.html".as_ptr()) }.is_null() {
        return Err("no frontend embedded. Rebuild with MORI_WITH_UI=1".into());
    }
    // SAFETY: this command runs before C main, before any SQLite use or worker.
    unsafe {
        cbm_alloc_init();
        cbm_log_init_from_env();
    }
    Server::bind(port)?.serve(port, &signals)
}

enum Command {
    Help,
    Serve(u16),
}

fn parse(args: &[&[u8]]) -> Result<Command, &'static str> {
    let value = match args {
        [] => return Ok(Command::Serve(DEFAULT_PORT)),
        [b"--help" | b"-h"] => return Ok(Command::Help),
        [b"--port", value] => *value,
        [flag] if flag.starts_with(b"--port=") => &flag[b"--port=".len()..],
        _ => return Err("expected --port <port> or --help"),
    };
    if !value.iter().all(u8::is_ascii_digit) {
        return Err("port must be an integer from 1 to 65535");
    }
    match std::str::from_utf8(value).ok().and_then(|p| p.parse().ok()) {
        Some(port) if port != 0 => Ok(Command::Serve(port)),
        _ => Err("port must be an integer from 1 to 65535"),
    }
}

pub(crate) fn run(args: &[&[u8]]) -> c_int {
    match parse(args) {
        Ok(Command::Help) => {
            println!(
                "Usage: mori ui [--port <port>]\n\nServe the read-only graph UI on 127.0.0.1 (default port {DEFAULT_PORT})."
            );
            0
        }
        Ok(Command::Serve(port)) => match start(port) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("mori ui: {error}");
                1
            }
        },
        Err(error) => {
            eprintln!("mori ui: {error}. See mori ui --help.");
            2
        }
    }
}
