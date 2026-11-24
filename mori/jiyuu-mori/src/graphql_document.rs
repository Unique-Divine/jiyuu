//! Byte-span facts for GraphQL operations, including embedded templates.

use std::collections::HashSet;
use std::ops::Range;

pub(super) type Span = Range<usize>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OperationKind {
    Query,
    Mutation,
    Subscription,
}

impl OperationKind {
    fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"query" => Some(Self::Query),
            b"mutation" => Some(Self::Mutation),
            b"subscription" => Some(Self::Subscription),
            _ => None,
        }
    }

    fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Query => b"query",
            Self::Mutation => b"mutation",
            Self::Subscription => b"subscription",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Definition {
    Operation {
        kind: OperationKind,
        name: Span,
        shorthand: bool,
    },
    Fragment {
        name: Span,
        type_condition: Span,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SelectionKind {
    Field(Span),
    FragmentSpread(Span),
    InlineFragment(Span),
    Interpolated,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Selection {
    pub(super) depth: usize,
    pub(super) kind: SelectionKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct InvalidDocument;

pub(super) struct Parser<'a> {
    source: &'a [u8],
    offset: usize,
}

fn is_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_name_byte(byte: u8) -> bool {
    is_name_start(byte) || byte.is_ascii_digit()
}

impl<'a> Parser<'a> {
    pub(super) fn new(source: &'a [u8]) -> Self {
        Self { source, offset: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.source.get(self.offset).copied()
    }

    fn starts_with(&self, prefix: &[u8]) -> bool {
        self.source[self.offset..].starts_with(prefix)
    }

    fn take(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.offset += 1;
            true
        } else {
            false
        }
    }

    fn name(&mut self) -> Span {
        let start = self.offset;
        if self.peek().is_some_and(is_name_start) {
            self.offset += 1;
            while self.peek().is_some_and(is_name_byte) {
                self.offset += 1;
            }
        }
        start..self.offset
    }

    fn skip_quoted(&mut self, quote: u8) {
        while let Some(byte) = self.peek() {
            self.offset += 1;
            match byte {
                b'\\' if self.peek().is_some() => self.offset += 1,
                byte if byte == quote => break,
                _ => {}
            }
        }
    }

    fn skip_until(&mut self, end: impl Fn(u8) -> bool) {
        while self.peek().is_some_and(|byte| !end(byte)) {
            self.offset += 1;
        }
    }

    fn skip_block_string(&mut self) {
        self.offset += 3;
        while self.peek().is_some() {
            if self.starts_with(b"\"\"\"") {
                let escaped = self.source[self.offset - 1] == b'\\';
                self.offset += 3;
                if !escaped {
                    break;
                }
            } else {
                self.offset += 1;
            }
        }
    }

    fn skip_opaque(&mut self) -> bool {
        match self.peek() {
            Some(b'#') => self.skip_until(|byte| matches!(byte, b'\n' | b'\r')),
            Some(b'"') if self.starts_with(b"\"\"\"") => {
                self.skip_block_string()
            }
            Some(b'"') => {
                self.offset += 1;
                self.skip_quoted(b'"');
            }
            Some(b'$') if self.starts_with(b"${") => self.skip_interpolation(),
            _ => return false,
        }
        true
    }

    fn skip_trivia(&mut self, allow_interpolation: bool) -> bool {
        let mut interpolated = false;
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\n' | b'\r' | b',') => self.offset += 1,
                Some(b'\\') if self.offset + 1 < self.source.len() => {
                    self.offset += 2;
                }
                Some(b'#') => {
                    self.skip_opaque();
                }
                Some(b'$') if allow_interpolation && self.starts_with(b"${") => {
                    self.skip_interpolation();
                    interpolated = true;
                }
                _ => return interpolated,
            }
        }
    }

    fn skip_arguments(&mut self) {
        if !self.take(b'(') {
            return;
        }
        let mut depth = 1;
        while let Some(byte) = self.peek() {
            if self.skip_opaque() {
                continue;
            }
            self.offset += 1;
            match byte {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                return;
            }
        }
    }

    fn skip_directives(&mut self, allow_interpolation: bool) {
        self.skip_trivia(allow_interpolation);
        while self.take(b'@') {
            self.name();
            self.skip_trivia(allow_interpolation);
            self.skip_arguments();
            self.skip_trivia(allow_interpolation);
        }
    }

    /// Read the next definition header. The caller then reads its selection
    /// set. The legacy fixed-buffer encoder can stop at an oversized header.
    pub(super) fn next_header(
        &mut self,
        allow_shorthand: bool,
    ) -> Result<Option<Definition>, InvalidDocument> {
        self.skip_trivia(true);
        if self.peek().is_none() {
            return Ok(None);
        }
        if allow_shorthand && self.peek() == Some(b'{') {
            return Ok(Some(Definition::Operation {
                kind: OperationKind::Query,
                name: self.offset..self.offset,
                shorthand: true,
            }));
        }
        let keyword = self.name();
        self.skip_trivia(true);
        let name = self.name();
        self.skip_trivia(true);
        let definition = if &self.source[keyword.clone()] == b"fragment" {
            if name.is_empty() || &self.source[self.name()] != b"on" {
                return Err(InvalidDocument);
            }
            self.skip_trivia(true);
            let type_condition = self.name();
            if type_condition.is_empty() {
                return Err(InvalidDocument);
            }
            Definition::Fragment {
                name,
                type_condition,
            }
        } else {
            let kind = OperationKind::from_name(&self.source[keyword])
                .ok_or(InvalidDocument)?;
            self.skip_arguments();
            Definition::Operation {
                kind,
                name,
                shorthand: false,
            }
        };
        self.skip_directives(true);
        if self.peek() != Some(b'{') {
            return Err(InvalidDocument);
        }
        Ok(Some(definition))
    }

    /// Read a selection set without recursion. Depth gives each selection's
    /// position relative to the definition's opening brace.
    pub(super) fn selections(
        &mut self,
        mut emit: impl FnMut(Selection),
    ) -> Result<(), InvalidDocument> {
        if !self.take(b'{') {
            return Err(InvalidDocument);
        }
        let mut depth = 0;
        loop {
            let interpolated = self.skip_trivia(true);
            if self.take(b'}') {
                if depth == 0 {
                    return Ok(());
                }
                depth -= 1;
                continue;
            }
            let kind = if interpolated && self.peek() == Some(b'{') {
                SelectionKind::Interpolated
            } else if self.starts_with(b"...") {
                self.offset += 3;
                self.skip_trivia(false);
                let name = self.name();
                let kind = if &self.source[name.clone()] == b"on" {
                    self.skip_trivia(false);
                    SelectionKind::InlineFragment(self.name())
                } else if name.is_empty() {
                    SelectionKind::InlineFragment(name)
                } else {
                    SelectionKind::FragmentSpread(name)
                };
                self.skip_directives(false);
                kind
            } else {
                let mut field = self.name();
                if field.is_empty() {
                    return Err(InvalidDocument);
                }
                self.skip_trivia(false);
                if self.take(b':') {
                    self.skip_trivia(false);
                    field = self.name();
                    if field.is_empty() {
                        return Err(InvalidDocument);
                    }
                    self.skip_trivia(false);
                }
                self.skip_arguments();
                self.skip_directives(false);
                SelectionKind::Field(field)
            };
            emit(Selection { depth, kind });
            if self.take(b'{') {
                depth += 1;
            }
        }
    }

    // A slash after a JS operand is division. A slash at the start of an
    // expression can open a regex whose braces do not close the interpolation.
    fn regex_allowed(&self, begin: usize) -> bool {
        let expr = &self.source[begin..self.offset];
        let Some(last) = expr
            .iter()
            .rposition(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
        else {
            return true;
        };
        let byte = expr[last];
        if b"(,=:[!&|?{};+-*%<>~^".contains(&byte) {
            return true;
        }
        if !is_name_byte(byte) {
            return false;
        }
        let start = expr[..last]
            .iter()
            .rposition(|&byte| !is_name_byte(byte))
            .map_or(0, |index| index + 1);
        matches!(
            &expr[start..=last],
            b"return"
                | b"typeof"
                | b"case"
                | b"void"
                | b"delete"
                | b"in"
                | b"of"
                | b"new"
                | b"instanceof"
                | b"throw"
                | b"yield"
                | b"await"
                | b"else"
                | b"do"
        )
    }

    fn skip_regex(&mut self) {
        self.offset += 1;
        let mut in_class = false;
        while let Some(byte) = self.peek().filter(|&byte| byte != b'\n') {
            self.offset += 1;
            match byte {
                b'\\' if self.peek().is_some() => self.offset += 1,
                b'[' => in_class = true,
                b']' => in_class = false,
                b'/' if !in_class => break,
                _ => {}
            }
        }
        while self.peek().is_some_and(is_name_byte) {
            self.offset += 1;
        }
    }

    fn skip_interpolation(&mut self) {
        #[derive(Clone, Copy)]
        enum Frame {
            Expression { begin: usize, braces: usize },
            Template,
        }

        // Keep the native tokenizer's bound for nested templates.
        let mut frames = [Frame::Template; 32];
        self.offset += 2;
        frames[0] = Frame::Expression {
            begin: self.offset,
            braces: 1,
        };
        let mut count = 1;
        while let Some(byte) = self.peek().filter(|_| count > 0) {
            let room = count < frames.len();
            match frames[count - 1] {
                Frame::Template => match byte {
                    b'\\' if self.offset + 1 < self.source.len() => {
                        self.offset += 2
                    }
                    b'`' => {
                        count -= 1;
                        self.offset += 1;
                    }
                    b'$' if room && self.starts_with(b"${") => {
                        self.offset += 2;
                        frames[count] = Frame::Expression {
                            begin: self.offset,
                            braces: 1,
                        };
                        count += 1;
                    }
                    _ => self.offset += 1,
                },
                Frame::Expression { begin, mut braces } => match byte {
                    b'\'' | b'"' => {
                        self.offset += 1;
                        self.skip_quoted(byte);
                    }
                    b'`' if room => {
                        frames[count] = Frame::Template;
                        count += 1;
                        self.offset += 1;
                    }
                    b'/' if self.starts_with(b"//") => {
                        self.skip_until(|byte| byte == b'\n')
                    }
                    b'/' if self.starts_with(b"/*") => {
                        self.offset += 2;
                        while self.peek().is_some() && !self.starts_with(b"*/") {
                            self.offset += 1;
                        }
                        if self.starts_with(b"*/") {
                            self.offset += 2;
                        }
                    }
                    b'/' if self.regex_allowed(begin) => self.skip_regex(),
                    _ => {
                        if byte == b'{' {
                            braces += 1;
                        } else if byte == b'}' {
                            braces -= 1;
                        }
                        frames[count - 1] = Frame::Expression { begin, braces };
                        count -= usize::from(braces == 0);
                        self.offset += 1;
                    }
                },
            }
        }
    }
}

/// Project the operation facts onto Mori's existing field-path wire format.
/// The route pass uses only root fields and their direct children.
pub(super) fn signature(
    source: &[u8],
    require_keyword: bool,
    capacity: usize,
    shorthand: &mut bool,
) -> Option<Vec<u8>> {
    const NAME_LIMIT: usize = 127;
    const PATH_LIMIT: usize = 512;
    let mut parser = Parser::new(source);
    let mut output = Vec::new();
    *shorthand = false;
    while let Some(header) = parser.next_header(output.is_empty()).ok()? {
        let Definition::Operation {
            kind,
            name,
            shorthand: short,
        } = header
        else {
            parser.selections(|_| {}).ok()?;
            continue;
        };
        *shorthand |= short;
        if require_keyword && short {
            return None;
        }
        let name = &source[name];
        let name = &name[..name.len().min(NAME_LIMIT)];
        let kind = kind.as_bytes();
        // The native writer reserves a newline and a NUL for every header.
        if capacity - output.len() < kind.len() + name.len() + 4 {
            break;
        }
        if !output.is_empty() {
            output.push(b'\n');
        }
        output.extend_from_slice(kind);
        output.push(b'|');
        output.extend_from_slice(name);
        output.push(b'|');
        let first_path = output.len();
        let mut parent = None;
        let mut seen = HashSet::new();
        parser
            .selections(|Selection { depth, kind }| {
                let field = match kind {
                    SelectionKind::Field(span)
                        if !source[span.clone()].starts_with(b"__") =>
                    {
                        Some(&source[span])
                    }
                    _ => None,
                };
                let prefix = match depth {
                    0 => {
                        parent = field;
                        &[][..]
                    }
                    1 => match parent {
                        Some(parent) => parent,
                        None => return,
                    },
                    _ => return,
                };
                let Some(field) = field else {
                    return;
                };
                let qualified = !prefix.is_empty();
                let path_len =
                    prefix.len() + usize::from(qualified) + field.len();
                let needs_comma = output.len() != first_path;
                if prefix.len() + field.len() + 2 > PATH_LIMIT
                    || path_len + usize::from(needs_comma) + 1
                        > capacity - output.len()
                    || !seen.insert((prefix, field))
                {
                    return;
                }
                if needs_comma {
                    output.push(b',');
                }
                if qualified {
                    output.extend_from_slice(prefix);
                    output.push(b'.');
                }
                output.extend_from_slice(field);
            })
            .ok()?;
    }
    (!output.is_empty()).then_some(output)
}

#[cfg(test)]
#[path = "graphql_document_tests.rs"]
mod tests;
