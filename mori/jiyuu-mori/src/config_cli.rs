//! The config command uses the same settings storage as the daemon.

use std::ffi::CStr;
use std::io::{self, Write};

struct Key<'a> {
    name: &'a CStr,
    default: &'a CStr,
    help: &'a CStr,
}

enum OpenError {
    MissingHome,
    Database,
}

trait Config {
    fn read(&mut self, key: &Key<'_>) -> Vec<u8>;

    // Returns whether the daemon needs a restart. The backend reports errors.
    fn write(&mut self, key: &Key<'_>, value: Option<&CStr>)
    -> Result<bool, ()>;
}

fn field(out: &mut impl Write, bytes: &[u8], width: usize) -> io::Result<()> {
    const SPACES: &[u8; 25] = b"                         ";
    out.write_all(bytes)?;
    out.write_all(&SPACES[..width.saturating_sub(bytes.len())])
}

fn help(out: &mut impl Write, keys: &[Key<'_>]) -> io::Result<()> {
    out.write_all(
        concat!(
            "Usage: mori config <command> [args]\n\n",
            "Commands:\n",
            "  list             Show all config values\n",
            "  get <key>        Get a config value\n",
            "  set <key> <val>  Set a config value\n",
            "  reset <key>      Reset a key to default\n\n",
            "Config keys:\n",
        )
        .as_bytes(),
    )?;
    for key in keys {
        out.write_all(b"  ")?;
        field(out, key.name.to_bytes(), 25)?;
        out.write_all(b"  default=")?;
        field(out, key.default.to_bytes(), 10)?;
        out.write_all(b"  ")?;
        out.write_all(key.help.to_bytes())?;
        out.write_all(b"\n")?;
    }
    Ok(())
}

fn run<B: Config>(
    args: &[&CStr],
    keys: &[Key<'_>],
    open: impl FnOnce() -> Result<B, OpenError>,
    out: &mut impl Write,
    err: &mut impl Write,
) -> io::Result<i32> {
    let [command, rest @ ..] = args else {
        help(out, keys)?;
        return Ok(0);
    };
    let command = command.to_bytes();
    if matches!(command, b"--help" | b"-h") {
        help(out, keys)?;
        return Ok(0);
    }

    // Keep the standalone command's error ordering and database side effects.
    let mut cfg = match open() {
        Ok(cfg) => cfg,
        Err(error) => {
            err.write_all(match error {
                OpenError::MissingHome => {
                    b"error: HOME not set (use USERPROFILE on Windows)\n"
                }
                OpenError::Database => b"error: cannot open config database\n",
            })?;
            return Ok(1);
        }
    };

    let usage: &[u8] = match command {
        b"list" | b"ls" => {
            out.write_all(b"Configuration:\n")?;
            for key in keys {
                out.write_all(b"  ")?;
                field(out, key.name.to_bytes(), 25)?;
                out.write_all(b" = ")?;
                field(out, &cfg.read(key), 10)?;
                out.write_all(b"\n")?;
            }
            return Ok(0);
        }
        b"get" => b"Usage: config get <key>\n",
        b"set" => b"Usage: config set <key> <value>\n",
        b"reset" => b"Usage: config reset <key>\n",
        _ => {
            err.write_all(b"Unknown config command: ")?;
            err.write_all(command)?;
            err.write_all(b"\n")?;
            return Ok(1);
        }
    };

    let [name, values @ ..] = rest else {
        err.write_all(usage)?;
        return Ok(1);
    };
    let value = values.first().copied();
    if command == b"set" && value.is_none() {
        err.write_all(usage)?;
        return Ok(1);
    }
    let Some(key) = keys.iter().find(|key| key.name == *name) else {
        err.write_all(b"error: unknown config key: ")?;
        err.write_all(name.to_bytes())?;
        err.write_all(b"\nKnown keys:")?;
        for key in keys {
            err.write_all(b" ")?;
            err.write_all(key.name.to_bytes())?;
        }
        err.write_all(b"\n")?;
        return Ok(1);
    };

    match command {
        b"get" => {
            out.write_all(&cfg.read(key))?;
            out.write_all(b"\n")?;
        }
        b"set" => {
            let value = value.expect("set requires a value");
            let Ok(restart) = cfg.write(key, Some(value)) else {
                return Ok(1);
            };
            out.write_all(name.to_bytes())?;
            out.write_all(b" = ")?;
            out.write_all(value.to_bytes())?;
            out.write_all(b"\n")?;
            if restart {
                out.write_all(
                    b"  (restart the daemon for this to take effect)\n",
                )?;
            }
        }
        _ => {
            if cfg.write(key, None).is_err() {
                return Ok(1);
            }
            out.write_all(name.to_bytes())?;
            out.write_all(b" reset to default\n")?;
        }
    }
    Ok(0)
}

#[cfg(feature = "native-engine-abi")]
mod native {
    use std::ffi::{c_char, c_int, c_void};
    use std::{ptr, slice};

    use super::*;

    #[repr(C)]
    struct NativeKey {
        name: *const c_char,
        default: *const c_char,
        help: *const c_char,
    }

    impl From<&Key<'_>> for NativeKey {
        fn from(key: &Key<'_>) -> Self {
            Self {
                name: key.name.as_ptr(),
                default: key.default.as_ptr(),
                help: key.help.as_ptr(),
            }
        }
    }

    unsafe extern "C" {
        fn cbm_config_cli_keys(count: *mut usize) -> *const NativeKey;
        fn cbm_get_home_dir() -> *const c_char;
        fn cbm_config_cli_open() -> *mut c_void;
        fn cbm_config_close(cfg: *mut c_void);
        fn cbm_config_cli_read(
            cfg: *mut c_void,
            key: *const NativeKey,
            ui_value: *mut c_char,
            ui_value_size: usize,
        ) -> *const c_char;
        fn cbm_config_cli_write(
            cfg: *mut c_void,
            key: *const NativeKey,
            value: *const c_char,
        ) -> c_int;
        fn cbm_config_cli_output(
            error_stream: bool,
            bytes: *const u8,
            len: usize,
        ) -> usize;
        fn cbm_config_cli_flush(error_stream: bool) -> c_int;
    }

    struct NativeConfig(ptr::NonNull<c_void>);

    impl NativeConfig {
        fn open() -> Result<Self, OpenError> {
            // SAFETY: The C entry point has completed process setup.
            if unsafe { cbm_get_home_dir() }.is_null() {
                return Err(OpenError::MissingHome);
            }
            // SAFETY: Native open returns an owned connection or NULL.
            ptr::NonNull::new(unsafe { cbm_config_cli_open() })
                .map(Self)
                .ok_or(OpenError::Database)
        }
    }

    impl Drop for NativeConfig {
        fn drop(&mut self) {
            // SAFETY: NativeConfig owns this connection.
            unsafe { cbm_config_close(self.0.as_ptr()) };
        }
    }

    impl Config for NativeConfig {
        fn read(&mut self, key: &Key<'_>) -> Vec<u8> {
            // The UI values are a boolean or a decimal port.
            let mut ui_value = [0; 32];
            // SAFETY: The configuration, key, and output buffer live through
            // the call. Copy the result before another read can replace it.
            unsafe {
                let value = cbm_config_cli_read(
                    self.0.as_ptr(),
                    &NativeKey::from(key),
                    ui_value.as_mut_ptr(),
                    ui_value.len(),
                );
                CStr::from_ptr(value).to_bytes().to_vec()
            }
        }

        fn write(
            &mut self,
            key: &Key<'_>,
            value: Option<&CStr>,
        ) -> Result<bool, ()> {
            let value = value.map_or(ptr::null(), CStr::as_ptr);
            // SAFETY: C borrows the connection, key, and optional value.
            match unsafe {
                cbm_config_cli_write(
                    self.0.as_ptr(),
                    &NativeKey::from(key),
                    value,
                )
            } {
                0 => Ok(false),
                1 => Ok(true),
                _ => Err(()),
            }
        }
    }

    // The C entry point and native test runner must use the same CRT streams.
    // In particular, Windows _dup2 changes the CRT fd without changing the
    // Win32 standard handle that Rust's std::io would write to.
    struct NativeStream {
        error_stream: bool,
    }

    impl Write for NativeStream {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            // SAFETY: C synchronously reads the supplied byte slice.
            Ok(unsafe {
                cbm_config_cli_output(
                    self.error_stream,
                    bytes.as_ptr(),
                    bytes.len(),
                )
            })
        }

        fn flush(&mut self) -> io::Result<()> {
            // SAFETY: C only flushes its own standard stream.
            if unsafe { cbm_config_cli_flush(self.error_stream) } == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn cbm_cmd_config(
        argc: c_int,
        argv: *const *const c_char,
    ) -> c_int {
        // SAFETY: C main passes its live argument suffix. The keys and their
        // strings are static data owned by the native settings implementation.
        let (args, keys) = unsafe {
            let mut count = 0;
            let keys =
                slice::from_raw_parts(cbm_config_cli_keys(&mut count), count)
                    .iter()
                    .map(|key| Key {
                        name: CStr::from_ptr(key.name),
                        default: CStr::from_ptr(key.default),
                        help: CStr::from_ptr(key.help),
                    })
                    .collect::<Vec<_>>();
            let argv = if argc > 0 && !argv.is_null() {
                slice::from_raw_parts(argv, argc as usize)
            } else {
                &[]
            };
            let args = argv
                .iter()
                .map(|&arg| CStr::from_ptr(arg))
                .collect::<Vec<_>>();
            (args, keys)
        };
        let mut output = NativeStream {
            error_stream: false,
        };
        let mut error = NativeStream { error_stream: true };
        let result =
            run(&args, &keys, NativeConfig::open, &mut output, &mut error)
                .and_then(|status| {
                    output.flush()?;
                    error.flush()?;
                    Ok(status)
                });
        result.unwrap_or(1)
    }
}

#[cfg(test)]
mod tests;
