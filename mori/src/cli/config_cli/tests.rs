use super::*;

const KEYS: &[Key<'_>] = &[
    Key {
        name: c"auto_watch",
        default: c"true",
        help: c"Enable watching",
    },
    Key {
        name: c"ui_port",
        default: c"9749",
        help: c"UI port",
    },
];

type Writes = Vec<(Vec<u8>, Option<Vec<u8>>)>;

#[derive(Default)]
struct FakeConfig {
    writes: Writes,
    reject: bool,
}

impl Config for &mut FakeConfig {
    fn read(&mut self, key: &Key<'_>) -> Vec<u8> {
        key.default.to_bytes().to_vec()
    }

    fn write(
        &mut self,
        key: &Key<'_>,
        value: Option<&CStr>,
    ) -> Result<bool, ()> {
        if self.reject {
            return Err(());
        }
        self.writes.push((
            key.name.to_bytes().to_vec(),
            value.map(|value| value.to_bytes().to_vec()),
        ));
        Ok(key.name == c"ui_port")
    }
}

fn invoke<B: Config>(
    args: &[&CStr],
    open: impl FnOnce() -> Result<B, OpenError>,
) -> (i32, Vec<u8>, Vec<u8>) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let status = run(args, KEYS, open, &mut out, &mut err).unwrap();
    (status, out, err)
}

#[test]
fn help_does_not_open_settings() {
    for args in [&[][..], &[c"--help"], &[c"-h", c"ignored"]] {
        let (status, out, err) =
            invoke(args, || -> Result<&mut FakeConfig, OpenError> {
                panic!("help opened settings");
            });
        assert_eq!(status, 0);
        assert!(err.is_empty());
        assert_eq!(
            out,
            b"Usage: mori config <command> [args]\n\nCommands:\n\
              \x20 list             Show all config values\n\
              \x20 get <key>        Get a config value\n\
              \x20 set <key> <val>  Set a config value\n\
              \x20 reset <key>      Reset a key to default\n\nConfig keys:\n\
              \x20 auto_watch                 default=true        Enable watching\n\
              \x20 ui_port                    default=9749        UI port\n",
        );
    }
}

#[test]
fn list_alias_and_get_use_the_same_backend() {
    let mut cfg = FakeConfig::default();
    for command in [c"list", c"ls"] {
        let (status, out, err) = invoke(&[command], || Ok(&mut cfg));
        assert_eq!(status, 0);
        assert!(err.is_empty());
        assert_eq!(
            out,
            b"Configuration:\n\x20 auto_watch                = true      \n\x20 ui_port                   = 9749      \n"
        );
    }
    assert_eq!(
        invoke(&[c"get", c"auto_watch", c"ignored"], || Ok(&mut cfg)),
        (0, b"true\n".to_vec(), vec![]),
    );
}

#[test]
fn mutations_preserve_bytes_and_only_set_prints_the_restart_notice() {
    let mut cfg = FakeConfig::default();
    for (args, expected) in [
        (
            &[c"set", c"auto_watch", c"\xff"][..],
            &b"auto_watch = \xff\n"[..],
        ),
        (
            &[c"set", c"ui_port", c"08080", c"ignored"],
            b"ui_port = 08080\n  (restart the daemon for this to take effect)\n",
        ),
        (
            &[c"reset", c"ui_port", c"ignored"],
            b"ui_port reset to default\n",
        ),
    ] {
        assert_eq!(
            invoke(args, || Ok(&mut cfg)),
            (0, expected.to_vec(), vec![])
        );
    }
    assert_eq!(
        cfg.writes,
        [
            (b"auto_watch".to_vec(), Some(vec![0xff])),
            (b"ui_port".to_vec(), Some(b"08080".to_vec())),
            (b"ui_port".to_vec(), None),
        ]
    );
    cfg.reject = true;
    assert_eq!(
        invoke(&[c"set", c"ui_port", c"0"], || Ok(&mut cfg)),
        (1, vec![], vec![]),
    );
}

#[test]
fn errors_preserve_diagnostic_order() {
    let mut cfg = FakeConfig::default();
    for (args, expected) in [
        (&[c"get"][..], &b"Usage: config get <key>\n"[..]),
        (&[c"set", c"bad"], b"Usage: config set <key> <value>\n"),
        (&[c"reset"], b"Usage: config reset <key>\n"),
        (
            &[c"get", c"\xff"],
            b"error: unknown config key: \xff\nKnown keys: auto_watch ui_port\n",
        ),
        (&[c"bad"], b"Unknown config command: bad\n"),
    ] {
        assert_eq!(
            invoke(args, || Ok(&mut cfg)),
            (1, vec![], expected.to_vec())
        );
    }
    for (error, expected) in [
        (
            OpenError::MissingHome,
            &b"error: HOME not set (use USERPROFILE on Windows)\n"[..],
        ),
        (OpenError::Database, b"error: cannot open config database\n"),
    ] {
        assert_eq!(
            invoke(&[c"bad"], || Err::<&mut FakeConfig, _>(error)),
            (1, vec![], expected.to_vec()),
        );
    }
    assert!(cfg.writes.is_empty());
}
