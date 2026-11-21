//! Serialize the graph properties of a GraphQL server Route.

/// Return the Route's NUL-terminated properties, preserving the C writer's
/// key order and escaping for existing graph comparisons.
pub(crate) fn route_properties(
    return_type: &[u8],
    reason: &[u8],
    deprecated: bool,
    ambiguous: bool,
) -> Vec<u8> {
    let mut out = br#"{"source":"graphql_schema","schema_type":"#.to_vec();
    push_string(&mut out, return_type);
    out.extend_from_slice(br#","deprecated":"#);
    out.extend_from_slice(if deprecated { b"true" } else { b"false" });
    out.extend_from_slice(br#","deprecation_reason":"#);
    push_string(&mut out, reason);
    if ambiguous {
        out.extend_from_slice(br#","ambiguous":true"#);
    }
    out.extend_from_slice(b"}\0");
    out
}

// yyjson's ALLOW_INVALID_UNICODE mode preserves the source bytes, uses
// short escapes when defined, and emits uppercase hex for other controls.
fn push_string(out: &mut Vec<u8>, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push(b'"');
    for &byte in bytes {
        match byte {
            b'"' => out.extend_from_slice(br#"\""#),
            b'\\' => out.extend_from_slice(br"\\"),
            b'\n' => out.extend_from_slice(br"\n"),
            b'\r' => out.extend_from_slice(br"\r"),
            b'\t' => out.extend_from_slice(br"\t"),
            0x08 => out.extend_from_slice(br"\b"),
            0x0c => out.extend_from_slice(br"\f"),
            0x00..=0x1f => out.extend_from_slice(&[
                b'\\',
                b'u',
                b'0',
                b'0',
                HEX[usize::from(byte >> 4)],
                HEX[usize::from(byte & 0xf)],
            ]),
            _ => out.push(byte),
        }
    }
    out.push(b'"');
}

#[cfg(test)]
mod tests {
    use super::route_properties;

    #[test]
    fn route_properties_match_c_wire_format() {
        assert_eq!(
            route_properties(b"Int", b"", false, false),
            b"{\"source\":\"graphql_schema\",\"schema_type\":\"Int\",\"deprecated\":false,\"deprecation_reason\":\"\"}\0",
        );
        assert_eq!(
            route_properties(b"[Int!]!", b"\"/\\\x0f\x08\x0c\n\r\t\xff", true, true),
            b"{\"source\":\"graphql_schema\",\"schema_type\":\"[Int!]!\",\"deprecated\":true,\"deprecation_reason\":\"\\\"/\\\\\\u000F\\b\\f\\n\\r\\t\xff\",\"ambiguous\":true}\0",
        );
    }
}
