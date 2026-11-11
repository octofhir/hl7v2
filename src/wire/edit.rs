use super::{Error, ErrorKind, Message, Presence, Span, escape, valid_path};
use std::{borrow::Cow, collections::BTreeMap};

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
    kind: Kind,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Replace,
    Remove,
    Create(usize),
}
type Group<'a> = Vec<(&'a [usize], &'a Patch)>;

/// Sparse replacements and explicit creation of positions within existing segments.
#[doc = include_str!("../../docs/editing.md")]
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
        self.replace(path, edit, false)
    }
    /// Fill missing positions with empty siblings. Does not create segments or change delimiters.
    pub fn set_or_create(&mut self, path: &[usize], edit: Edit) -> Result<&mut Self, Error> {
        self.replace(path, edit, true)
    }
    fn replace(&mut self, path: &[usize], edit: Edit, create: bool) -> Result<&mut Self, Error> {
        if let Some(span) = &self.message.index.unterminated {
            return Err(Error::new(ErrorKind::Escape, span.clone()));
        }
        if !valid_path(path) || (path[0] == 1 && path[1] <= 2) {
            return Err(Error::new(ErrorKind::Path, 0..0));
        }
        let value = self.message.value(path);
        if value.presence() == Presence::Missing {
            if matches!(edit, Edit::Unset) {
                // Cancelling an exact pending creation never removes source siblings.
                if self
                    .patches
                    .get(path)
                    .is_some_and(|p| matches!(p.kind, Kind::Create(_)))
                    && let Some(patch) = self.patches.remove(path)
                {
                    self.bytes -= patch.bytes.len();
                }
                return Ok(self);
            }
            if !create {
                return Err(Error::new(ErrorKind::Missing, 0..0));
            }
        }
        let mut span = value.span().unwrap_or(0..0);
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
        let mut kind = Kind::Replace;
        if value.presence() == Presence::Missing {
            (span, kind) = self.creation(path)?;
        }
        let max = self.message.options.max_bytes;
        let bytes = match edit {
            Edit::Value(bytes) => escape::encode(&bytes, self.message.delimiters(), max)?,
            Edit::Empty => Vec::new(),
            Edit::Null => b"\"\"".to_vec(),
            Edit::Unset => {
                let parent = &path[..path.len() - 1];
                let count = self.count(parent);
                let number = path[path.len() - 1];
                if number != count || (path.len() > 2 && number == 1) {
                    return Err(Error::new(ErrorKind::InteriorUnset, span));
                }
                let mut after = path.to_vec();
                after[path.len() - 1] = number
                    .checked_add(1)
                    .ok_or_else(|| Error::new(ErrorKind::Limit, span.clone()))?;
                if self
                    .patches
                    .range(after..)
                    .next()
                    .is_some_and(|(key, _)| key.starts_with(parent))
                {
                    return Err(Error::new(ErrorKind::EditConflict, span));
                }
                span.start -= 1; // Include the preceding separator; the position is the final sibling.
                kind = Kind::Remove;
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
        self.patches.insert(key, Patch { span, bytes, kind });
        Ok(self)
    }
    fn count(&self, parent: &[usize]) -> usize {
        if parent.len() == 1 {
            self.message.index.segments[parent[0] - 1].fields.len()
        } else {
            self.message.value(parent).child_count()
        }
    }
    fn creation(&self, path: &[usize]) -> Result<(Span, Kind), Error> {
        let segment = self
            .message
            .index
            .segments
            .get(path[0] - 1)
            .ok_or_else(|| Error::new(ErrorKind::Missing, 0..0))?;
        if path[1..]
            .iter()
            .any(|&number| number > self.message.options.max_nodes)
        {
            return Err(Error::new(ErrorKind::Limit, 0..0));
        }
        for length in 2..=path.len() {
            let value = self.message.value(&path[..length]);
            if value.presence() == Presence::Null && length < path.len() {
                return Err(Error::new(ErrorKind::Null, value.span().unwrap_or(0..0)));
            }
            if value.presence() == Presence::Missing {
                let parent = &path[..length - 1];
                let end = if parent.len() == 1 {
                    segment.span.end
                } else {
                    self.message
                        .value(parent)
                        .span()
                        .ok_or_else(|| Error::new(ErrorKind::Missing, 0..0))?
                        .end
                };
                let mut last = parent.to_vec();
                last.push(self.count(parent));
                if self
                    .patches
                    .get(&last)
                    .is_some_and(|patch| patch.kind == Kind::Remove)
                {
                    return Err(Error::new(ErrorKind::EditConflict, end..end));
                }
                return Ok((end..end, Kind::Create(length - 1)));
            }
        }
        Err(Error::new(ErrorKind::Path, 0..0))
    }
    pub fn serialize(&self) -> Result<Vec<u8>, Error> {
        let mut patches = Vec::new();
        let mut groups: BTreeMap<&[usize], Group<'_>> = BTreeMap::new();
        for (path, patch) in &self.patches {
            if let Kind::Create(depth) = patch.kind {
                groups
                    .entry(&path[..depth])
                    .or_default()
                    .push((path, patch));
            } else {
                patches.push((
                    patch.span.clone(),
                    Cow::Borrowed(patch.bytes.as_slice()),
                    path.as_slice(),
                ));
            }
        }
        let mut nodes = self
            .message
            .options
            .max_nodes
            .saturating_sub(self.message.node_count());
        let mut separators = self.message.options.max_bytes - self.bytes;
        for (parent, group) in groups {
            if let Some(&(first, patch)) = group.first() {
                let bytes = self.render(parent, &group, &mut nodes, &mut separators)?;
                patches.push((patch.span.clone(), Cow::Owned(bytes), first));
            }
        }
        patches.sort_unstable_by(|a, b| (a.0.start, a.0.end, a.2).cmp(&(b.0.start, b.0.end, b.2)));
        let mut output = Vec::new();
        let mut cursor = 0;
        let max = self.message.options.max_bytes;
        for (span, bytes, _) in patches {
            if span.start < cursor {
                return Err(Error::new(ErrorKind::EditConflict, span));
            }
            escape::append(&mut output, &self.message.bytes[cursor..span.start], max)?;
            escape::append(&mut output, &bytes, max)?;
            cursor = span.end;
        }
        escape::append(&mut output, &self.message.bytes[cursor..], max)?;
        Ok(output)
    }
    // Sorted sparse paths share implicit first children; only divergent sibling tails need separators.
    fn render(
        &self,
        parent: &[usize],
        group: &Group<'_>,
        nodes: &mut usize,
        budget: &mut usize,
    ) -> Result<Vec<u8>, Error> {
        let mut output = Vec::new();
        let mut previous = parent;
        let delimiters = self.message.delimiters().separators();
        for &(path, patch) in group {
            let depth = path
                .iter()
                .zip(previous)
                .take_while(|(a, b)| a == b)
                .count();
            let before = if previous.len() == parent.len() {
                self.count(parent)
            } else {
                *previous
                    .get(depth)
                    .ok_or_else(|| Error::new(ErrorKind::EditConflict, patch.span.clone()))?
            };
            for (level, &number) in path.iter().enumerate().skip(depth) {
                let count = number
                    .checked_sub(if level == depth { before } else { 1 })
                    .ok_or_else(|| Error::new(ErrorKind::EditConflict, patch.span.clone()))?;
                let added = count
                    .checked_mul(5 - level)
                    .ok_or_else(|| Error::new(ErrorKind::Limit, patch.span.clone()))?;
                if added > *nodes || count > *budget {
                    return Err(Error::new(ErrorKind::Limit, patch.span.clone()));
                }
                *nodes -= added;
                *budget -= count;
                output.resize(output.len() + count, delimiters[level - 1]);
            }
            escape::append(&mut output, &patch.bytes, self.message.options.max_bytes)?;
            previous = path;
        }
        Ok(output)
    }
    pub fn finish(self) -> Result<Message, Error> {
        Message::from_shared(self.serialize()?.into(), self.message.options)
    }
}
