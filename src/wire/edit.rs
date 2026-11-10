use super::{Error, ErrorKind, Message, Presence, Span, escape, valid_path};
use std::collections::BTreeMap;

/// Values are unescaped scalar bytes. Null is never inferred from a supplied value.
pub enum Edit {
    Value(Vec<u8>),
    Empty,
    Null,
    Unset,
}
struct Patch {
    span: Span,
    bytes: Vec<u8>,
}

/// Replacements of existing positions. Creation of missing positions needs a future builder.
pub struct Editor<'a> {
    message: &'a Message,
    patches: BTreeMap<Vec<usize>, Patch>,
    bytes: usize,
}
impl<'a> Editor<'a> {
    pub(super) fn new(message: &'a Message) -> Self {
        Self {
            message,
            patches: BTreeMap::new(),
            bytes: 0,
        }
    }
    pub fn set(&mut self, path: &[usize], edit: Edit) -> Result<&mut Self, Error> {
        if let Some(span) = &self.message.index.unterminated {
            return Err(Error::new(ErrorKind::Escape, span.clone()));
        }
        if !valid_path(path) || (path[0] == 1 && path[1] <= 2) {
            return Err(Error::new(ErrorKind::Path, 0..0));
        }
        let value = self.message.value(path);
        if value.presence() == Presence::Missing {
            return if matches!(edit, Edit::Unset) {
                Ok(self)
            } else {
                Err(Error::new(ErrorKind::Missing, 0..0))
            };
        }
        let mut span = value
            .span()
            .ok_or_else(|| Error::new(ErrorKind::Missing, 0..0))?;
        for length in 2..path.len() {
            if self.patches.contains_key(&path[..length]) {
                return Err(Error::new(ErrorKind::EditConflict, span));
            }
        }
        let key = path.to_vec();
        if self
            .patches
            .range(key.clone()..)
            .next()
            .is_some_and(|(other, _)| other.len() > key.len() && other.starts_with(path))
        {
            return Err(Error::new(ErrorKind::EditConflict, span));
        }
        let max = self.message.options.max_bytes;
        let bytes = match edit {
            Edit::Value(bytes) => escape::encode(&bytes, self.message.delimiters(), max)?,
            Edit::Empty => Vec::new(),
            Edit::Null => b"\"\"".to_vec(),
            Edit::Unset => {
                let count = if path.len() == 2 {
                    self.message.index.segments[path[0] - 1].fields.len()
                } else {
                    self.message.value(&path[..path.len() - 1]).child_count()
                };
                let number = path[path.len() - 1];
                if number != count || (path.len() > 2 && number == 1) {
                    return Err(Error::new(ErrorKind::InteriorUnset, span));
                }
                span.start -= 1; // Include the preceding separator; the position is the final sibling.
                Vec::new()
            }
        };
        let old = self.patches.get(path).map_or(0, |patch| patch.bytes.len());
        let remaining = self.bytes - old;
        if bytes.len() > max.saturating_sub(remaining)
            || (old == 0
                && !self.patches.contains_key(path)
                && self.patches.len() >= self.message.options.max_nodes)
        {
            return Err(Error::new(ErrorKind::Limit, span));
        }
        self.bytes = remaining + bytes.len();
        self.patches.insert(key, Patch { span, bytes });
        Ok(self)
    }
    pub fn serialize(&self) -> Result<Vec<u8>, Error> {
        let mut patches: Vec<_> = self.patches.values().collect();
        patches.sort_unstable_by_key(|patch| (patch.span.start, patch.span.end));
        let mut output = Vec::new();
        let mut cursor = 0;
        let max = self.message.options.max_bytes;
        for patch in patches {
            if patch.span.start < cursor {
                return Err(Error::new(ErrorKind::EditConflict, patch.span.clone()));
            }
            escape::append(
                &mut output,
                &self.message.bytes[cursor..patch.span.start],
                max,
            )?;
            escape::append(&mut output, &patch.bytes, max)?;
            cursor = patch.span.end;
        }
        escape::append(&mut output, &self.message.bytes[cursor..], max)?;
        Ok(output)
    }
    pub fn finish(self) -> Result<Message, Error> {
        Message::from_shared(self.serialize()?.into(), self.message.options)
    }
}
