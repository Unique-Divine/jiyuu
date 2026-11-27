use super::json::{Doc, Value};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Dialect {
    Event,
    Copilot,
    Hermes,
    Qoder,
    Kimi,
    Devin,
    Cline,
    Gemini,
    Qwen,
    Factory,
    Augment,
}

impl Dialect {
    pub(super) fn named(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"copilot" => Self::Copilot,
            b"hermes" => Self::Hermes,
            b"qoder" => Self::Qoder,
            b"kimi" => Self::Kimi,
            b"devin" => Self::Devin,
            b"cline" => Self::Cline,
            b"gemini" => Self::Gemini,
            b"qwen" => Self::Qwen,
            b"factory" => Self::Factory,
            b"augment" => Self::Augment,
            _ => return None,
        })
    }

    pub(super) fn from_optional(name: Option<&[u8]>) -> Option<Self> {
        match name {
            None | Some(b"") => Some(Self::Event),
            Some(name) => Self::named(name),
        }
    }

    pub(super) fn lifecycle_event(self, event: &[u8]) -> bool {
        match self {
            Self::Hermes => event == b"pre_llm_call",
            Self::Factory => event == b"SessionStart",
            Self::Gemini | Self::Augment => false,
            Self::Kimi => event == b"UserPromptSubmit",
            Self::Devin => matches!(
                event,
                b"SessionStart" | b"UserPromptSubmit" | b"PostCompaction"
            ),
            Self::Cline => matches!(
                event,
                b"TaskStart"
                    | b"TaskResume"
                    | b"UserPromptSubmit"
                    | b"PreCompact"
            ),
            _ => matches!(event, b"SessionStart" | b"SubagentStart"),
        }
    }

    pub(super) fn invocation(self, forced: Option<&[u8]>) -> bool {
        match forced {
            None => self != Self::Copilot,
            Some(b"PostToolUse") if self == Self::Copilot => true,
            Some(event) => self.lifecycle_event(event),
        }
    }
}

pub(super) fn event_name(root: Value<'_>) -> Option<&[u8]> {
    root.field(c"hook_event_name")
        .or_else(|| root.field(c"hookEventName"))
}

pub(super) struct ToolCall<'a> {
    pub(super) event: Option<&'a [u8]>,
    pub(super) tool: Option<&'a [u8]>,
    pub(super) input: Value<'a>,
    input_doc: Option<Doc>,
}

impl<'a> ToolCall<'a> {
    pub(super) fn read(
        root: Value<'a>,
        forced: Option<&'a [u8]>,
        dialect: Dialect,
    ) -> Self {
        let mut call = Self {
            event: event_name(root),
            tool: root.field(c"tool_name"),
            input: root.get(c"tool_input"),
            input_doc: None,
        };
        if dialect == Dialect::Copilot {
            call.event = forced;
            call.tool =
                call.tool.or_else(|| root.field(c"toolName")).map(|tool| {
                    match tool {
                        b"bash" | b"powershell" => b"Bash",
                        b"grep" | b"rg" => b"Grep",
                        b"glob" => b"Glob",
                        _ => tool,
                    }
                });
            if call.input.missing() {
                call.input = root.get(c"toolArgs");
            }
            call.input_doc = call.input.raw_string().and_then(Doc::read);
        }
        call
    }

    pub(super) fn search_token(&self) -> Option<Vec<u8>> {
        let input = self.input_doc.as_ref().map_or(self.input, Doc::root);
        let pattern = if self.tool == Some(b"Bash") {
            bash_pattern(input.field(c"command")?)?
        } else {
            input.field(c"pattern")?.to_vec()
        };
        extract_token(&pattern).map(<[u8]>::to_vec)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolEvent {
    Search,
    Coverage,
}

pub(super) fn tool_event(
    dialect: Dialect,
    event: &[u8],
    tool: &[u8],
) -> Option<ToolEvent> {
    use Dialect::*;
    match (dialect, event, tool) {
        (Event, b"PreToolUse", b"Grep" | b"Glob" | b"Bash")
        | (Copilot, b"PostToolUse", b"Grep" | b"Glob" | b"Bash") => {
            Some(ToolEvent::Search)
        }
        (Event | Qoder | Factory, b"PostToolUse", b"Read")
        | (Gemini, b"AfterTool", b"read_file")
        | (Qwen, b"PostToolUse", b"ReadFile")
        | (Augment, b"PostToolUse", b"view") => Some(ToolEvent::Coverage),
        _ => None,
    }
}

pub(super) fn is_graph_tool(tool: &[u8]) -> bool {
    let Some(name) = tool.strip_prefix(b"mcp__") else {
        return false;
    };
    let Some(separator) = name.windows(2).position(|pair| pair == b"__") else {
        return false;
    };
    if separator + 2 == name.len() {
        return false;
    }
    let server = &name[..separator];
    // Keep these in sync with CBM_MCP_SERVER_KEYS in cli/mori_names.h.
    [b"mori-graph".as_slice(), b"codebase-memory-mcp"]
        .iter()
        .any(|key| {
            server == *key
                || server.strip_prefix(b"plugin_").is_some_and(|plugin| {
                    plugin.strip_suffix(*key).is_some_and(|prefix| {
                        prefix.len() > 1 && prefix.ends_with(b"_")
                    })
                })
        })
}

fn identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn identifier_byte(byte: u8) -> bool {
    identifier_start(byte) || byte.is_ascii_digit()
}

pub(super) fn extract_token(mut pattern: &[u8]) -> Option<&[u8]> {
    let mut best = &pattern[..0];
    while let Some(start) =
        pattern.iter().position(|&byte| identifier_start(byte))
    {
        pattern = &pattern[start..];
        let len = pattern
            .iter()
            .position(|&byte| !identifier_byte(byte))
            .unwrap_or(pattern.len());
        if len > best.len() {
            best = &pattern[..len];
        }
        pattern = &pattern[len..];
    }
    (best.len() >= 4).then(|| &best[..best.len().min(96)])
}

// This is the hook's search recognizer. It never executes the tool command.
fn tokenize(mut command: &[u8]) -> Vec<Vec<u8>> {
    fn space(byte: u8) -> bool {
        matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
    }

    let mut tokens = Vec::new();
    while !command.is_empty() && tokens.len() < 32 {
        let mut token = Vec::new();
        let mut quote = None;
        while let Some((&byte, rest)) = command.split_first() {
            command = rest;
            if quote.is_none() && space(byte) {
                break;
            }
            match (quote, byte) {
                (None, b'\'' | b'"') => quote = Some(byte),
                (Some(delimiter), _) if delimiter == byte => quote = None,
                (Some(b'\''), _) => token.push(byte),
                (_, b'\\') if !rest.is_empty() => {
                    if quote.is_none() || b"\\\"$`".contains(&rest[0]) {
                        token.push(rest[0]);
                        command = &rest[1..];
                    } else {
                        token.push(byte);
                    }
                }
                _ => token.push(byte),
            }
            // The C reader truncated quoted bytes. An overlong unquoted
            // token wedged its cursor and could stall a daemon request.
            if token.len() > 255 {
                if quote.is_none() {
                    return Vec::new();
                }
                token.truncate(255);
            }
        }
        if !token.is_empty() {
            tokens.push(token);
        }
    }
    tokens
}

fn env_assignment(token: &[u8]) -> bool {
    token
        .iter()
        .position(|&byte| byte == b'=')
        .is_some_and(|end| {
            let name = &token[..end];
            name.first().is_some_and(|&byte| identifier_start(byte))
                && name.iter().all(|&byte| identifier_byte(byte))
        })
}

pub(super) fn bash_pattern(command: &[u8]) -> Option<Vec<u8>> {
    let tokens = tokenize(command);
    let mut tokens = tokens.iter().map(Vec::as_slice).peekable();
    let mut rtk = false;
    let bin = loop {
        match tokens.next()? {
            token if env_assignment(token) => {}
            b"env" | b"nice" | b"time" | b"command" => {}
            b"rtk" => rtk = true,
            b"tokf" if tokens.peek() == Some(&b"run".as_slice()) => {
                tokens.next();
            }
            bin => break bin,
        }
    };
    let (grep, value_flags): (bool, &[u8]) = match bin {
        b"git" if tokens.next() == Some(b"grep") => (true, b"ABCmdD"),
        b"grep" | b"egrep" | b"fgrep" => (true, b"ABCmdD"),
        b"rg" => (false, b"ABCmtTgMP"),
        b"ag" => (false, b"ABCmpG"),
        b"ack" | b"ugrep" | b"ug" => (false, b"ABCmdD"),
        _ => return None,
    };

    let mut pattern = None;
    let mut explicit = 0;
    let mut end_of_flags = false;
    while let Some(token) = tokens.next() {
        if end_of_flags || !token.starts_with(b"-") || token == b"-" {
            if pattern.is_some() {
                break;
            }
            pattern = Some(token);
            continue;
        }
        if token == b"--" {
            end_of_flags = true;
            continue;
        }
        if let Some(option) = token.strip_prefix(b"--") {
            let (name, value) = match option
                .iter()
                .position(|&byte| byte == b'=')
            {
                Some(index) => (&option[..index], Some(&option[index + 1..])),
                None => (option, None),
            };
            match name {
                b"regexp" | b"pattern" => {
                    pattern = value.or_else(|| tokens.next());
                    explicit += 1;
                }
                b"file" => return None,
                _ => {}
            }
            continue;
        }
        for (index, &flag) in token[1..].iter().enumerate() {
            let suffix = &token[index + 2..];
            match flag {
                b'e' => {
                    if !suffix.is_empty() {
                        pattern = Some(suffix);
                    } else if let Some(next) = tokens.next() {
                        pattern = Some(next);
                    }
                    explicit += 1;
                    break;
                }
                b'f' => return None,
                b'l' if rtk && grep => return None,
                flag if value_flags.contains(&flag) => {
                    if suffix.is_empty() {
                        tokens.next();
                    }
                    break;
                }
                _ => {}
            }
        }
    }
    pattern
        .filter(|pattern| explicit <= 1 && !pattern.is_empty())
        .map(<[u8]>::to_vec)
}
