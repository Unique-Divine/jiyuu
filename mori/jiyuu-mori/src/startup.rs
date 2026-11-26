//! Classify process roles and select commands from platform argv.

use std::ffi::{CStr, c_char, c_int};

// Keep these discriminants in sync with cbm_daemon_process_role_t in
// daemon/bootstrap.h and main_subcommand_t in main.c, respectively.
#[repr(C)]
pub(crate) enum ProcessRole {
    Invalid = 0,
    Stateless = 1,
    Daemon = 2,
    Worker = 3,
    McpClient = 4,
    LocalCli = 5,
    HookClient = 6,
    DaemonCtl = 7,
}

#[repr(C)]
pub(crate) enum Subcommand {
    None = 0,
    Version = 1,
    Help = 2,
    AllowRoot = 3,
    Cli = 4,
    HookAugment = 5,
    TestImpact = 6,
    Install = 7,
    Uninstall = 8,
    Update = 9,
    Config = 10,
}

impl Subcommand {
    fn parse(arg: &[u8]) -> Self {
        match arg {
            b"--version" => Self::Version,
            b"--help" | b"-h" => Self::Help,
            b"allow-root" => Self::AllowRoot,
            b"cli" => Self::Cli,
            b"hook-augment" => Self::HookAugment,
            b"test-impact" => Self::TestImpact,
            b"install" => Self::Install,
            b"uninstall" => Self::Uninstall,
            b"update" => Self::Update,
            b"config" => Self::Config,
            _ => Self::None,
        }
    }
}

/// Borrow the original C strings, including non-UTF-8 bytes on POSIX.
///
/// # Safety
/// When argv is non-null, it must address argc slots that outlive the iterator.
/// Each slot must be null or point to a valid NUL-terminated string.
pub unsafe fn argv_bytes<'a>(
    argc: c_int,
    argv: *const *mut c_char,
) -> impl Iterator<Item = &'a [u8]> + Clone {
    let argv = if argc > 0 && !argv.is_null() {
        // SAFETY: the caller guarantees argc initialized slots.
        unsafe { std::slice::from_raw_parts(argv, argc as usize) }
    } else {
        &[]
    };
    argv.iter().map(|&arg| {
        if arg.is_null() {
            return b"".as_slice();
        }
        // SAFETY: Each non-null entry is a valid NUL-terminated string.
        unsafe { CStr::from_ptr(arg) }.to_bytes()
    })
}

fn positive_worker_budget(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .try_fold(0usize, |value, &byte| {
            if byte.is_ascii_digit() {
                value.checked_mul(10)?.checked_add(usize::from(byte - b'0'))
            } else {
                None
            }
        })
        .is_some_and(|value| value > 0)
}

// Syntactic gate for the supervisor's re-exec. C verifies the actual build
// identity and worker options again before admitting the worker.
fn worker_args_exact<'a>(mut args: impl Iterator<Item = &'a [u8]>) -> bool {
    let [
        Some(b"cli"),
        Some(b"--index-worker"),
        Some(b"--index-worker-build"),
        Some(fingerprint),
        Some(b"index_repository"),
        Some(payload),
        Some(b"--response-out"),
        Some(response),
    ] = std::array::from_fn(|_| args.next())
    else {
        return false;
    };
    if fingerprint.len() != 64
        || !fingerprint
            .iter()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        || payload.is_empty()
        || response.is_empty()
    {
        return false;
    }

    let mut flags = args.peekable();
    if flags
        .next_if_eq(&b"--index-worker-memory-budget-bytes".as_slice())
        .is_some()
        && !flags.next().is_some_and(positive_worker_budget)
    {
        return false;
    }
    flags.next_if_eq(&b"--index-worker-single-thread".as_slice());
    for option in [
        b"--index-worker-marker".as_slice(),
        b"--index-worker-quarantine",
        b"--index-worker-publish-fence",
    ] {
        if flags.next_if_eq(&option).is_some()
            && !flags.next().is_some_and(|value| !value.is_empty())
        {
            return false;
        }
    }
    flags.next().is_none()
}

fn process_role<'a>(
    mut args: impl Iterator<Item = &'a [u8]> + Clone,
) -> ProcessRole {
    if args.next().is_none_or(<[u8]>::is_empty) {
        return ProcessRole::Invalid;
    }

    const DAEMON_INTERNAL: &[u8] = b"--cbm-daemon-internal";
    if args.clone().any(|arg| arg == DAEMON_INTERNAL) {
        let exact = args.next() == Some(DAEMON_INTERNAL)
            && matches!(
                (args.next(), args.next()),
                (None, None) | (Some(b"--cbm-daemon-permanent"), None)
            );
        return if exact {
            ProcessRole::Daemon
        } else {
            ProcessRole::Invalid
        };
    }
    if args.clone().any(|arg| arg == b"--index-worker") {
        return if worker_args_exact(args) {
            ProcessRole::Worker
        } else {
            ProcessRole::Invalid
        };
    }

    // Stop at the first public mode. CLI arguments can contain command names
    // or --version. Help anywhere after cli, config, or daemon is the legacy
    // exception and bypasses daemon coordination.
    while let Some(arg) = args.next() {
        let role = match Subcommand::parse(arg) {
            Subcommand::Cli | Subcommand::Config => ProcessRole::LocalCli,
            Subcommand::HookAugment => return ProcessRole::HookClient,
            Subcommand::None if arg == b"daemon" => ProcessRole::DaemonCtl,
            Subcommand::None => continue,
            _ => return ProcessRole::Stateless,
        };
        return if args
            .any(|arg| matches!(Subcommand::parse(arg), Subcommand::Help))
        {
            ProcessRole::Stateless
        } else {
            role
        };
    }
    ProcessRole::McpClient
}

fn select_subcommand<'a>(
    args: impl Iterator<Item = &'a [u8]>,
) -> Option<(usize, Subcommand)> {
    args.skip(1).enumerate().find_map(|(index, arg)| {
        let command = Subcommand::parse(arg);
        (!matches!(command, Subcommand::None)).then_some((index + 2, command))
    })
}

/// The informational commands dispatched directly by the linked product.
#[derive(Debug, PartialEq, Eq)]
pub enum InformationalCommand {
    Version,
    Help,
}

/// Select an informational command using the same role and subcommand rules
/// as the native entry point.
pub fn informational_command<'a>(
    args: impl Iterator<Item = &'a [u8]> + Clone,
) -> Option<InformationalCommand> {
    let command = match select_subcommand(args.clone())?.1 {
        Subcommand::Version => InformationalCommand::Version,
        Subcommand::Help => InformationalCommand::Help,
        _ => return None,
    };
    matches!(process_role(args), ProcessRole::Stateless).then_some(command)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_daemon_process_role(
    argc: c_int,
    argv: *const *mut c_char,
) -> ProcessRole {
    // SAFETY: C passes a live argv vector or NULL.
    process_role(unsafe { argv_bytes(argc, argv) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_main_subcommand(
    argc: c_int,
    argv: *const *mut c_char,
    next_arg: *mut c_int,
) -> Subcommand {
    // SAFETY: C passes a live argv vector or NULL.
    let args = unsafe { argv_bytes(argc, argv) };
    let (offset, command) = select_subcommand(args)
        .map_or((argc, Subcommand::None), |(offset, command)| {
            (offset as c_int, command)
        });
    // SAFETY: C supplies a writable int slot for the argument offset. The
    // selected offset is bounded by argc.
    unsafe { next_arg.write(offset) };
    command
}

#[cfg(test)]
mod tests {
    use super::InformationalCommand::{Help, Version};
    use super::informational_command;

    #[test]
    fn informational_dispatch_respects_role_and_command_precedence() {
        let cases: &[(&[&[u8]], _)] = &[
            (&[b"mori", b"--profile", b"--help", b"cli"], Some(Help)),
            (&[b"mori", b"--version", b"--help"], Some(Version)),
            (&[b"mori", b"\xff", b"-h"], Some(Help)),
            (
                &[b"mori", b"daemon", b"--version", b"--help"],
                Some(Version),
            ),
            (&[b"mori", b"daemon", b"--help"], Some(Help)),
            (&[b"mori", b"daemon", b"--version"], None),
            (&[b"mori", b"config", b"--help"], None),
            (&[b"mori", b"cli", b"list_projects", b"--help"], None),
            (&[b"mori", b"allow-root", b"--help"], None),
            (&[b"mori", b"--help", b"--index-worker"], None),
            (&[b"mori", b"--version", b"--cbm-daemon-internal"], None),
            (&[b"mori"], None),
        ];
        for (args, expected) in cases {
            assert_eq!(
                &informational_command(args.iter().copied()),
                expected,
                "{args:?}"
            );
        }
    }
}
