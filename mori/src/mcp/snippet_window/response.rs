//! Ordered tool payloads for get_code_snippet.

use super::{Window, source::physical_lines};
use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};
use std::{borrow::Cow, io};

#[cfg(feature = "native-engine-abi")]
pub(crate) mod native;

const BYTES_PER_TOKEN: usize = 4;

#[derive(Default)]
struct Object<'a>(Vec<(&'static str, Field<'a>)>);

#[derive(Serialize)]
#[serde(untagged)]
enum Field<'a> {
    Text(Text<'a>),
    Int(i32),
    Real(f64),
    Bool(bool),
    Rows(Vec<Object<'a>>),
    Names(Vec<Text<'a>>),
}

struct Text<'a>(Cow<'a, [u8]>);

impl Text<'_> {
    fn normalize(&mut self) {
        let bytes = &*self.0;
        // Match cbm_output_encode_text, including the reserved literal prefixes.
        // The legacy document normalizes in place on every budget probe.
        if std::str::from_utf8(bytes).is_err() {
            use std::fmt::Write as _;
            let mut encoded = String::from("@bytes:");
            for byte in bytes {
                write!(encoded, "{byte:02x}").unwrap();
            }
            self.0 = Cow::Owned(encoded.into_bytes());
        } else if bytes.starts_with(b"@bytes:") || bytes.starts_with(b"@utf8:") {
            self.0 = Cow::Owned([b"@utf8:".as_slice(), bytes].concat());
        }
    }
}

impl Serialize for Text<'_> {
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(
            std::str::from_utf8(&self.0).expect("normalized output text"),
        )
    }
}

impl<'a> Object<'a> {
    fn text(&mut self, key: &'static str, value: impl Into<Cow<'a, [u8]>>) {
        self.0.push((key, Field::Text(Text(value.into()))));
    }

    fn int(&mut self, key: &'static str, value: i32) {
        self.0.push((key, Field::Int(value)));
    }

    fn boolean(&mut self, key: &'static str, value: bool) {
        self.0.push((key, Field::Bool(value)));
    }

    fn remove(&mut self, key: &str) {
        self.0.retain(|(name, _)| *name != key);
    }

    fn json(&mut self) -> Vec<u8> {
        self.normalize();
        let mut json = Vec::new();
        self.serialize(&mut serde_json::Serializer::with_formatter(
            &mut json,
            JsonFormatter,
        ))
        .expect("write JSON to a vector");
        json
    }

    fn normalize(&mut self) {
        for (_, field) in &mut self.0 {
            match field {
                Field::Text(text) => text.normalize(),
                Field::Rows(rows) => rows.iter_mut().for_each(Self::normalize),
                Field::Names(names) => {
                    names.iter_mut().for_each(Text::normalize)
                }
                Field::Int(_) | Field::Real(_) | Field::Bool(_) => {}
            }
        }
    }
}

impl Serialize for Object<'_> {
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, value) in &self.0 {
            map.serialize_entry(name, value)?;
        }
        map.end()
    }
}

// yyjson uses uppercase hex digits for escaped JSON control bytes.
pub(crate) struct JsonFormatter;

impl serde_json::ser::Formatter for JsonFormatter {
    fn write_char_escape<W: io::Write + ?Sized>(
        &mut self,
        writer: &mut W,
        escape: serde_json::ser::CharEscape,
    ) -> io::Result<()> {
        if let serde_json::ser::CharEscape::AsciiControl(byte) = escape {
            write!(writer, "\\u{byte:04X}")
        } else {
            serde_json::ser::Formatter::write_char_escape(
                &mut serde_json::ser::CompactFormatter,
                writer,
                escape,
            )
        }
    }
}

struct Outline {
    offset: i32,
    total: i32,
}

impl Outline {
    fn append_paging(&self, result: &mut Object<'_>, returned: i32) {
        let next = self.offset.saturating_add(returned);
        result.int("members_returned", returned);
        result.boolean("members_has_more", next < self.total);
        if next < self.total {
            result.int("next_member_offset", next);
        }
    }
}

fn apply_source_prefix<'a>(
    result: &mut Object<'a>,
    source: &'a [u8],
    lines: usize,
    window: &Window,
) {
    for key in [
        "source",
        "source_omitted",
        "next_start_line",
        "continuation_requires_higher_budget",
        "continuation",
        "end_line",
        "source_truncated",
        "source_clipped",
        "truncation_reason",
        "original_end_line",
        "source_lines_returned",
    ] {
        result.remove(key);
    }
    if lines == 0 {
        result.boolean("source_omitted", true);
    } else {
        let len = physical_lines(source).take(lines).map(<[u8]>::len).sum();
        result.text("source", &source[..len]);
    }
    let lines = i32::try_from(lines).unwrap_or(i32::MAX);
    let end = window.start.saturating_add(lines.saturating_sub(1).max(0));
    result.int("end_line", end);
    result.boolean("source_truncated", true);
    result.boolean("source_clipped", true);
    result.text("truncation_reason", b"output_budget".as_slice());
    if lines > 0 {
        result.int("next_start_line", end.saturating_add(1));
    } else {
        result.boolean("continuation_requires_higher_budget", true);
        result.text("continuation",
            b"raise max_output_tokens; no complete source line fits the current budget".as_slice());
    }
    result.int("original_end_line", window.original_end);
    result.int("source_lines_returned", lines);
}

fn fit_source<'a>(
    result: &mut Object<'a>,
    source: &'a [u8],
    window: &Window,
    budget: usize,
    render: &impl Fn(&mut Object<'a>) -> Option<Vec<u8>>,
) -> Option<Vec<u8>> {
    let mut low = 0;
    let mut high = physical_lines(source).count();
    let mut best = None;
    while low <= high {
        let middle = low + (high - low) / 2;
        apply_source_prefix(result, source, middle, window);
        if let Some(candidate) =
            render(result).filter(|bytes| bytes.len() <= budget)
        {
            best = Some(candidate);
            low = middle + 1;
        } else if middle == 0 {
            break;
        } else {
            high = middle - 1;
        }
    }
    best.or_else(|| {
        apply_source_prefix(result, source, 0, window);
        render(result)
    })
}

fn fit_outline<'a>(
    result: &mut Object<'a>,
    outline: &Outline,
    initial: Vec<u8>,
    budget: usize,
    render: &impl Fn(&mut Object<'a>) -> Option<Vec<u8>>,
) -> Option<Vec<u8>> {
    result.text("truncation_reason", b"output_budget".as_slice());
    let mut payload = initial;
    while payload.len() > budget {
        let rows = result
            .0
            .iter_mut()
            .find_map(|(key, field)| match (*key, field) {
                ("members", Field::Rows(rows)) => Some(rows),
                _ => None,
            })
            .expect("outline member field");
        if rows.pop().is_none() {
            break;
        }
        let returned = rows.len() as i32;
        for key in ["members_returned", "members_has_more", "next_member_offset"]
        {
            result.remove(key);
        }
        outline.append_paging(result, returned);
        payload = render(result)?;
    }
    Some(payload)
}

fn budget_floor<'a>(max_output_tokens: i32) -> Object<'a> {
    let mut result = Object::default();
    result.text("source_mode", b"omitted".as_slice());
    result.boolean("truncated", true);
    result.text("truncation_reason", b"output_budget".as_slice());
    result.int("max_output_tokens", max_output_tokens);
    result.boolean("continuation_requires_higher_budget", true);
    result.text(
        "continuation",
        b"raise max_output_tokens; no partial line or identifier was emitted"
            .as_slice(),
    );
    result
}

fn render_bounded<'a>(
    mut result: Object<'a>,
    window: &Window,
    source: Option<&'a [u8]>,
    outline: Option<&Outline>,
    render: impl Fn(&mut Object<'a>) -> Option<Vec<u8>>,
) -> Option<Vec<u8>> {
    let payload = render(&mut result);
    if window.max_output_tokens == 0 {
        return payload;
    }
    let budget = window.max_output_tokens as usize * BYTES_PER_TOKEN;
    let payload = match payload {
        Some(bytes) if bytes.len() > budget => {
            if let Some(outline) = outline {
                fit_outline(&mut result, outline, bytes, budget, &render)
            } else if let Some(source) = source {
                fit_source(&mut result, source, window, budget, &render)
            } else {
                Some(bytes)
            }
        }
        payload => payload,
    };
    payload
        .filter(|bytes| bytes.len() <= budget)
        .or_else(|| render(&mut budget_floor(window.max_output_tokens)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Window {
        Window {
            original_start: 1,
            original_end: 8,
            start: 2,
            end: 3,
            max_output_tokens: 128,
            outline: false,
            clipped: true,
        }
    }

    #[test]
    fn snippet_json_preserves_order_and_wire_escapes() {
        let mut object = Object::default();
        object.text("z", b"quote\"/\\\n\x1b".as_slice());
        object.text("a", b"x\xff".as_slice());
        object.0.push(("p", Field::Real(1.0)));
        object.0.push(("members", Field::Rows(Vec::new())));
        object.0.push((
            "names",
            Field::Names(vec![Text(Cow::Borrowed(b"@bytes:literal"))]),
        ));
        assert_eq!(
            object.json(),
            br#"{"z":"quote\"/\\\n\u001B","a":"@bytes:78ff","p":1.0,"members":[],"names":["@utf8:@bytes:literal"]}"#,
        );
        assert_eq!(
            object.json(),
            br#"{"z":"quote\"/\\\n\u001B","a":"@utf8:@bytes:78ff","p":1.0,"members":[],"names":["@utf8:@utf8:@bytes:literal"]}"#,
        );
    }

    #[test]
    fn snippet_source_budget_moves_keys_and_keeps_complete_lines() {
        let window = window();
        let source = [b"keep\n".as_slice(), &[b'x'; 600]].concat();
        let mut result = Object::default();
        result.int("start_line", window.start);
        result.int("end_line", window.end);
        result.boolean("source_clipped", true);
        result.text("source", source.as_slice());
        result.int("callers", 0);
        let payload =
            render_bounded(result, &window, Some(&source), None, |obj| {
                Some(obj.json())
            })
            .unwrap();
        assert_eq!(
            payload,
            br#"{"start_line":2,"callers":0,"source":"keep\n","end_line":2,"source_truncated":true,"source_clipped":true,"truncation_reason":"output_budget","next_start_line":3,"original_end_line":8,"source_lines_returned":1}"#,
        );
    }

    #[test]
    fn snippet_outline_budget_keeps_paging_keys_and_uses_the_floor() {
        let mut window = window();
        window.outline = true;
        let outline = Outline {
            offset: 4,
            total: 9,
        };
        let long_name = [b'x'; 410];
        let members = (0..2)
            .map(|_| {
                let mut row = Object::default();
                row.text("qualified_name", long_name.as_slice());
                row
            })
            .collect();
        let mut result = Object::default();
        result.text("source_mode", b"outline".as_slice());
        result.0.push(("members", Field::Rows(members)));
        result.int("members_total", outline.total);
        outline.append_paging(&mut result, 2);
        result.boolean("full_source_available", true);
        let payload =
            render_bounded(result, &window, None, Some(&outline), |obj| {
                Some(obj.json())
            })
            .unwrap();
        assert_eq!(
            payload,
            br#"{"source_mode":"outline","members":[],"members_total":9,"full_source_available":true,"truncation_reason":"output_budget","members_returned":0,"members_has_more":true,"next_member_offset":4}"#,
        );
        let mut result = Object::default();
        result.text("name", &[b'x'; 600][..]);
        let payload =
            render_bounded(result, &window, None, None, |obj| Some(obj.json()))
                .unwrap();
        assert_eq!(
            payload,
            br#"{"source_mode":"omitted","truncated":true,"truncation_reason":"output_budget","max_output_tokens":128,"continuation_requires_higher_budget":true,"continuation":"raise max_output_tokens; no partial line or identifier was emitted"}"#,
        );
    }
}
