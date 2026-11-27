//! Schema-driven flag arguments for selected read-only `mori cli` tools.

use std::ffi::c_long;

use serde_json::Value;

fn decimal(value: &str) -> Option<c_long> {
    // The C CLI uses strtol in base 10. It accepts leading C whitespace
    // and saturates on overflow, but a trailing byte makes the value a string.
    let number = value
        .trim_start_matches(|ch: char| ch.is_ascii_whitespace() || ch == '\x0b');
    let digits = number.strip_prefix(['+', '-']).unwrap_or(number);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(number.parse().unwrap_or_else(|_| {
        if number.starts_with('-') {
            c_long::MIN
        } else {
            c_long::MAX
        }
    }))
}

fn build_args<'a>(
    schema: &str,
    flags: impl Iterator<Item = &'a str>,
) -> Option<Vec<u8>> {
    let schema: Value = serde_json::from_str(schema).ok()?;
    let properties = schema.get("properties")?.as_object()?;
    let mut flags = flags.peekable();
    let mut fields: Vec<(String, Value)> = Vec::new();
    while let Some(flag) = flags.next() {
        if flag == "--" {
            break;
        }
        let flag = flag.strip_prefix("--")?;
        let (key, value) = if let Some((key, value)) = flag.split_once('=') {
            (key, Some(value))
        } else if flags.peek().is_some_and(|next| !next.starts_with("--")) {
            (flag, flags.next())
        } else {
            (flag, None)
        };
        let key = key.replace('-', "_");
        let kind = properties.get(&key)?.get("type")?.as_str()?;
        let text = value.unwrap_or_default();
        if kind == "array" {
            let value = value?;
            let elements = if value.starts_with('[') {
                let Value::Array(elements) = serde_json::from_str(value).ok()?
                else {
                    return None;
                };
                // C copies array strings as NUL-terminated text. Leave
                // other JSON types and embedded NULs with the native parser.
                if !elements.iter().all(|element| {
                    element.as_str().is_some_and(|text| !text.contains('\0'))
                }) {
                    return None;
                }
                elements
            } else {
                vec![Value::from(value)]
            };
            if let Some((_, Value::Array(existing))) =
                fields.iter_mut().find(|(name, _)| *name == key)
            {
                existing.extend(elements);
            } else {
                fields.push((key, Value::Array(elements)));
            }
            continue;
        }
        let typed_value = match kind {
            // A bare C boolean flag is true. An explicit empty value is false.
            "boolean" => {
                Value::from(matches!(value, None | Some("true" | "1" | "yes")))
            }
            "integer" => {
                decimal(text).map_or_else(|| Value::from(text), Value::from)
            }
            "string" => Value::from(text),
            // A schema change can keep using C until Rust supports its type.
            _ => return None,
        };

        // The legacy CLI emits repeated keys in argv order. A JSON map would
        // silently drop duplicates before the daemon receives the request.
        fields.push((key, typed_value));
    }

    let mut args = vec![b'{'];
    for (key, value) in fields {
        if args.len() > 1 {
            args.push(b',');
        }
        serde_json::to_writer(&mut args, &key).ok()?;
        args.push(b':');
        serde_json::to_writer(&mut args, &value).ok()?;
    }
    args.push(b'}');
    Some(args)
}

#[cfg(all(feature = "c-backend", not(test)))]
pub(crate) mod c_args {
    use std::ffi::{CStr, CString, c_char, c_int};
    use std::{ptr, slice};

    type QueryArgsBuilder = unsafe extern "C" fn(
        *const c_char,
        c_int,
        *mut *mut c_char,
    ) -> *mut c_char;

    unsafe extern "C" {
        fn cbm_cli_set_query_args_builder(builder: QueryArgsBuilder);
        fn strdup(s: *const c_char) -> *mut c_char;
    }

    unsafe extern "C" fn build(
        schema: *const c_char,
        argc: c_int,
        argv: *mut *mut c_char,
    ) -> *mut c_char {
        // SAFETY: C supplies its live tool schema and all remaining argv slots.
        let schema = unsafe { CStr::from_ptr(schema) };
        let argv = unsafe { slice::from_raw_parts(argv, argc as usize) };
        let Ok(schema) = schema.to_str() else {
            return ptr::null_mut();
        };
        let flags = argv
            .iter()
            .map(|&arg| {
                // SAFETY: every live argv slot points to a C string.
                unsafe { CStr::from_ptr(arg) }.to_str()
            })
            .collect::<Result<Vec<_>, _>>();
        let Some(args) = flags.ok().and_then(|flags| {
            super::build_args(schema, flags.into_iter())
                .and_then(|json| CString::new(json).ok())
        }) else {
            return ptr::null_mut();
        };
        // SAFETY: strdup reads args before it drops and gives C a buffer
        // compatible with the caller's free.
        unsafe { strdup(args.as_ptr()) }
    }

    pub(crate) unsafe fn register() {
        // SAFETY: caller registers the process-global pointer before C starts.
        unsafe { cbm_cli_set_query_args_builder(build) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = r#"{
      "properties": {
        "query": {"type": "string"},
        "project": {"type": "string"},
        "max_rows": {"type": "integer"},
        "offset": {"type": "integer"},
        "include_details": {"type": "boolean"},
        "future_field": {"type": "array"}
      }
    }"#;

    fn parse(flags: &[&str]) -> Option<String> {
        build_args(SCHEMA, flags.iter().copied())
            .map(|json| String::from_utf8(json).unwrap())
    }

    #[test]
    fn preserves_aliases_values_and_duplicate_keys() {
        assert_eq!(
            parse(&[
                "--project=desk",
                "--query",
                "MATCH (n) WHERE n.name = \"Mori\" RETURN n",
                "--max-rows=+7",
                "--max_rows",
                "-4",
                "--offset",
                "--",
                "--ignored",
            ]),
            Some(
                r#"{"project":"desk","query":"MATCH (n) WHERE n.name = \"Mori\" RETURN n","max_rows":7,"max_rows":-4,"offset":""}"#
                    .into()
            ),
        );
        assert_eq!(
            parse(&["--query=--value", "--project"]),
            Some(r#"{"query":"--value","project":""}"#.into()),
        );
    }

    #[test]
    fn matches_c_integer_coercion() {
        assert_eq!(decimal(" \x0b\t+42"), Some(42));
        assert_eq!(decimal("999999999999999999999999"), Some(c_long::MAX));
        assert_eq!(decimal("-999999999999999999999999"), Some(c_long::MIN));
        assert_eq!(decimal("1 "), None);
        assert_eq!(decimal("0x2"), None);
        assert_eq!(decimal("999999999999999999999999x"), None);
        assert_eq!(parse(&["--offset=1 "]), Some(r#"{"offset":"1 "}"#.into()),);
    }

    #[test]
    fn matches_c_boolean_coercion() {
        assert_eq!(
            parse(&["--include-details"]),
            Some(r#"{"include_details":true}"#.into()),
        );
        for (value, expected) in [
            ("true", true),
            ("1", true),
            ("yes", true),
            ("", false),
            ("false", false),
            ("0", false),
            ("TRUE", false),
            ("arbitrary", false),
        ] {
            let flag = format!("--include_details={value}");
            let expected = format!(r#"{{"include_details":{expected}}}"#);
            assert_eq!(parse(&[&flag]), Some(expected), "{flag}");
        }
    }

    #[test]
    fn leaves_diagnostics_and_unhandled_array_values_to_c() {
        assert_eq!(parse(&["--max-row=1"]), None);
        assert_eq!(parse(&["--project=desk", "unexpected"]), None);
        for flag in ["--future-field", "--future-field=[", "--future-field=[1]"]
        {
            assert_eq!(parse(&[flag]), None, "{flag}");
        }
    }

    #[test]
    fn combines_repeated_array_flags_in_input_order() {
        assert_eq!(
            parse(&[
                "--future-field=[]",
                "--project=desk",
                "--future_field",
                "first",
                r#"--future-field=["second","third"]"#,
                "--future-field=",
                "--project=other",
            ]),
            Some(
                r#"{"future_field":["first","second","third",""],"project":"desk","project":"other"}"#
                    .into()
            ),
        );
    }
}
