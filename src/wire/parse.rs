use super::{
    Delimiters, Diagnostic, DiagnosticKind, Error, ErrorKind, Index, Node, ParseOptions,
    SegmentData, Span, escape,
};

pub(super) fn build(bytes: &[u8], options: ParseOptions) -> Result<Index, Error> {
    if bytes.len() > options.max_bytes {
        return Err(Error::new(ErrorKind::Limit, 0..bytes.len()));
    }
    let first_end = bytes
        .iter()
        .position(|b| matches!(b, b'\r' | b'\n'))
        .unwrap_or(bytes.len());
    let header = &bytes[..first_end];
    if !header.starts_with(b"MSH") || header.len() < 8 {
        return Err(Error::new(ErrorKind::Header, 0..first_end));
    }
    let field = header[3];
    let end = header[4..]
        .iter()
        .position(|&b| b == field)
        .map_or(first_end, |n| n + 4);
    let encoding = &header[4..end];
    if !matches!(encoding.len(), 4 | 5) {
        return Err(Error::new(ErrorKind::Delimiters, 3..end));
    }
    for (i, &byte) in header[3..end].iter().enumerate() {
        if !byte.is_ascii_punctuation() || byte == b'"' || header[3..3 + i].contains(&byte) {
            return Err(Error::new(ErrorKind::Delimiters, 3..end));
        }
    }
    let delimiters = Delimiters {
        field,
        encoding: [encoding[0], encoding[1], encoding[2], encoding[3]],
        truncation: encoding.get(4).copied(),
    };
    let mut builder = Builder {
        bytes,
        options,
        index: Index {
            delimiters,
            nodes: Vec::new(),
            segments: Vec::new(),
            diagnostics: Vec::new(),
            omitted: 0,
            unterminated: None,
        },
    };
    let mut start = 0;
    while start < bytes.len() {
        let stop = bytes[start..]
            .iter()
            .position(|b| matches!(b, b'\r' | b'\n'))
            .map_or(bytes.len(), |n| start + n);
        let next = stop
            + usize::from(stop < bytes.len())
            + usize::from(bytes.get(stop) == Some(&b'\r') && bytes.get(stop + 1) == Some(&b'\n'));
        if stop == bytes.len() {
            builder.report(DiagnosticKind::MissingTerminator, stop..stop, [0; 5]);
        } else if &bytes[stop..next] != b"\r" {
            if !options.compatible_line_endings {
                return Err(Error::new(ErrorKind::LineEnding, stop..next));
            }
            builder.report(DiagnosticKind::LineEnding, stop..next, [0; 5]);
        }
        builder.segment(start..stop, next, end)?;
        start = next;
    }
    Ok(builder.index)
}

struct Builder<'a> {
    bytes: &'a [u8],
    options: ParseOptions,
    index: Index,
}
impl Builder<'_> {
    fn report(&mut self, kind: DiagnosticKind, span: Span, path: [usize; 5]) {
        if kind == DiagnosticKind::UnterminatedEscape && self.index.unterminated.is_none() {
            self.index.unterminated = Some(span.clone());
        }
        if self.index.diagnostics.len() < self.options.max_diagnostics {
            self.index.diagnostics.push(Diagnostic { kind, span, path });
        } else {
            self.index.omitted += 1;
        }
    }
    // Reserve siblings before descending: every sibling list is a contiguous arena range.
    fn reserve(&mut self, spans: impl IntoIterator<Item = Span>) -> Result<Span, Error> {
        let start = self.index.nodes.len();
        for span in spans {
            if self.index.nodes.len() >= self.options.max_nodes {
                return Err(Error::new(ErrorKind::Limit, span));
            }
            self.index.nodes.push(Node {
                span,
                children: 0..0,
            });
        }
        Ok(start..self.index.nodes.len())
    }
    fn segment(&mut self, span: Span, end: usize, encoding_end: usize) -> Result<(), Error> {
        let text = &self.bytes[span.clone()];
        if text.len() < 3
            || !text[..3]
                .iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            || (text.len() > 3 && text[3] != self.index.delimiters.field)
        {
            return Err(Error::new(ErrorKind::Segment, span));
        }
        let id = [text[0], text[1], text[2]];
        let occurrence = self.index.segments.len() + 1;
        if occurrence > self.options.max_segments {
            return Err(Error::new(ErrorKind::Limit, span));
        }
        if occurrence > 1 && id == *b"MSH" {
            return Err(Error::new(ErrorKind::Header, span));
        }
        let start = self.index.nodes.len();
        let rest = if occurrence == 1 {
            self.reserve([3..4, 4..encoding_end])?;
            (encoding_end < span.end).then_some(encoding_end + 1)
        } else {
            (text.len() > 3).then_some(span.start + 4)
        };
        if let Some(rest) = rest {
            self.reserve(Parts::new(
                self.bytes,
                rest..span.end,
                self.index.delimiters.field,
                self.index.delimiters.encoding[2],
            ))?;
        }
        let fields = start..self.index.nodes.len();
        for (field, node) in fields.clone().enumerate() {
            if node >= 2 {
                self.fill(node, 0, [occurrence, field + 1, 0, 0, 0])?;
            }
        }
        self.index.segments.push(SegmentData {
            id,
            span,
            end,
            fields,
        });
        Ok(())
    }
    fn fill(&mut self, node: usize, depth: usize, path: [usize; 5]) -> Result<(), Error> {
        let span = self.index.nodes[node].span.clone();
        let delimiters = self.index.delimiters;
        if depth < 3 {
            let separator = delimiters.separators()[depth + 1];
            let children = self.reserve(Parts::new(
                self.bytes,
                span,
                separator,
                delimiters.encoding[2],
            ))?;
            self.index.nodes[node].children = children.clone();
            for (number, child) in children.enumerate() {
                let mut path = path;
                path[depth + 2] = number + 1;
                self.fill(child, depth + 1, path)?;
            }
        } else {
            for token in escape::tokens(
                &self.bytes[span.clone()],
                span.start,
                delimiters.encoding[2],
            ) {
                if let Some(kind) = escape::diagnostic(token.payload, delimiters) {
                    self.report(kind, token.span, path);
                }
            }
            if let Some(truncation) = delimiters.truncation {
                for part in Parts::new(self.bytes, span.clone(), truncation, delimiters.encoding[2])
                {
                    if part.end < span.end {
                        self.report(DiagnosticKind::Truncation, part.end..part.end + 1, path);
                    }
                }
            }
        }
        Ok(())
    }
}

struct Parts<'a> {
    bytes: &'a [u8],
    end: usize,
    next: Option<usize>,
    separator: u8,
    escape: u8,
}
impl<'a> Parts<'a> {
    fn new(bytes: &'a [u8], span: Span, separator: u8, escape: u8) -> Self {
        Self {
            bytes,
            end: span.end,
            next: Some(span.start),
            separator,
            escape,
        }
    }
}
impl Iterator for Parts<'_> {
    type Item = Span;
    fn next(&mut self) -> Option<Span> {
        let start = self.next?;
        let mut cursor = start;
        while cursor < self.end {
            if self.bytes[cursor] == self.escape
                && let Some(end) = escape::closing(self.bytes, cursor, self.end, self.escape)
            {
                cursor = end;
                continue;
            }
            if self.bytes[cursor] == self.separator {
                self.next = Some(cursor + 1);
                return Some(start..cursor);
            }
            cursor += 1;
        }
        self.next = None;
        Some(start..self.end)
    }
}
