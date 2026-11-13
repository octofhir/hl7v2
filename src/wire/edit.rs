use super::{Error, ErrorKind, Message, Presence, Span, escape, valid_path};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
mod segment;
pub use segment::SegmentBuilder;
struct Inserted {
    bytes: Vec<u8>,
    nodes: usize,
}

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

/// Sparse positional edits, non-MSH segment insertion and whole-segment removal.
#[doc = include_str!("../../docs/editing.md")]
pub struct Editor<'a> {
    message: &'a Message,
    patches: BTreeMap<Vec<usize>, Patch>,
    bytes: usize,
    insertions: BTreeMap<usize, Vec<Inserted>>,
    removals: BTreeSet<usize>,
    inserted: usize,
}
impl<'a> Editor<'a> {
    pub(super) fn new(message: &'a Message) -> Self {
        Self {
            message,
            patches: BTreeMap::new(),
            bytes: 0,
            insertions: BTreeMap::new(),
            removals: BTreeSet::new(),
            inserted: 0,
        }
    }
    /// Insert before an original one-based occurrence; N+1 appends. Same-anchor calls retain order.
    pub fn insert_segment(
        &mut self,
        before: usize,
        segment: SegmentBuilder<'_>,
    ) -> Result<&mut Self, Error> {
        self.check_escapes()?;
        if before < 2 || before > self.message.index.segments.len() + 1 {
            return Err(Error::new(ErrorKind::Path, 0..0));
        }
        if !Arc::ptr_eq(&self.message.index, &segment.message.index) {
            return Err(Error::new(ErrorKind::EditConflict, 0..0));
        }
        if self.inserted >= self.message.options.max_segments {
            return Err(Error::new(ErrorKind::Limit, 0..0));
        }
        let (bytes, nodes) = segment.encode()?;
        if bytes.len() > self.message.options.max_bytes.saturating_sub(self.bytes) {
            return Err(Error::new(ErrorKind::Limit, 0..0));
        }
        self.bytes += bytes.len();
        self.inserted += 1;
        self.insertions
            .entry(before)
            .or_default()
            .push(Inserted { bytes, nodes });
        Ok(self)
    }
    /// Remove an original segment and its own terminator. MSH is protected; repeated removal is idempotent.
    pub fn remove_segment(&mut self, occurrence: usize) -> Result<&mut Self, Error> {
        self.check_escapes()?;
        if occurrence < 2 {
            return Err(Error::new(ErrorKind::Path, 0..0));
        }
        let segment = self
            .message
            .segment(occurrence)
            .ok_or_else(|| Error::new(ErrorKind::Missing, 0..0))?;
        if self
            .patches
            .range(vec![occurrence]..)
            .next()
            .is_some_and(|(p, _)| p[0] == occurrence)
        {
            return Err(Error::new(ErrorKind::EditConflict, segment.span()));
        }
        self.removals.insert(occurrence);
        Ok(self)
    }
    fn check_escapes(&self) -> Result<(), Error> {
        if let Some(span) = &self.message.index.unterminated {
            return Err(Error::new(ErrorKind::Escape, span.clone()));
        }
        Ok(())
    }
    pub fn set(&mut self, path: &[usize], edit: Edit) -> Result<&mut Self, Error> {
        self.replace(path, edit, false)
    }
    /// Fill missing positions with empty siblings. Does not create segments or change delimiters.
    pub fn set_or_create(&mut self, path: &[usize], edit: Edit) -> Result<&mut Self, Error> {
        self.replace(path, edit, true)
    }
    fn replace(&mut self, path: &[usize], edit: Edit, create: bool) -> Result<&mut Self, Error> {
        self.check_escapes()?;
        if !valid_path(path) || (path[0] == 1 && path[1] <= 2) {
            return Err(Error::new(ErrorKind::Path, 0..0));
        }
        let value = self.message.value(path);
        if self.removals.contains(&path[0]) {
            return Err(Error::new(
                ErrorKind::EditConflict,
                value.span().unwrap_or(0..0),
            ));
        }
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
        if self.inserted
            > self
                .message
                .options
                .max_segments
                .saturating_sub(self.message.index.segments.len() - self.removals.len())
        {
            return Err(Error::new(ErrorKind::Limit, 0..0));
        }
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
        for inserted in self.insertions.values().flatten() {
            nodes = nodes
                .checked_sub(inserted.nodes)
                .ok_or_else(|| Error::new(ErrorKind::Limit, 0..0))?;
        }
        let mut separators = self.message.options.max_bytes - self.bytes;
        for (parent, group) in groups {
            if let Some(&(first, patch)) = group.first() {
                let bytes = render(
                    parent,
                    &group,
                    self.count(parent),
                    self.message,
                    &mut nodes,
                    &mut separators,
                )?;
                patches.push((patch.span.clone(), Cow::Owned(bytes), first));
            }
        }
        // Anchor paths sort after any tail field creation on an unterminated original final segment,
        // and zero-length insertion spans sort before deletion of the target segment.
        let anchors: Vec<_> = self.insertions.keys().map(|&n| [n]).collect();
        for ((&before, inserted), anchor) in self.insertions.iter().zip(&anchors) {
            let offset = self
                .message
                .index
                .segments
                .get(before - 1)
                .map_or(self.message.bytes.len(), |s| s.span.start);
            let mut bytes = Vec::new();
            if before == self.message.index.segments.len() + 1
                && !self.removals.contains(&(before - 1))
                && self
                    .message
                    .index
                    .segments
                    .last()
                    .is_some_and(|s| s.end == s.span.end)
            {
                escape::append(&mut bytes, b"\r", self.message.options.max_bytes)?;
            }
            for segment in inserted {
                escape::append(&mut bytes, &segment.bytes, self.message.options.max_bytes)?;
            }
            patches.push((offset..offset, Cow::Owned(bytes), anchor.as_slice()));
        }
        let removed: Vec<_> = self.removals.iter().map(|&n| [n]).collect();
        for path in &removed {
            let s = &self.message.index.segments[path[0] - 1];
            patches.push((s.span.start..s.end, Cow::Borrowed(&[]), path.as_slice()));
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
    pub fn finish(self) -> Result<Message, Error> {
        Message::from_shared(self.serialize()?.into(), self.message.options)
    }
}

// Sorted sparse paths share implicit first children; only divergent sibling tails need separators.
fn render(
    parent: &[usize],
    group: &Group<'_>,
    first_count: usize,
    message: &Message,
    nodes: &mut usize,
    budget: &mut usize,
) -> Result<Vec<u8>, Error> {
    let mut output = Vec::new();
    let mut previous = parent;
    let delimiters = message.delimiters().separators();
    for &(path, patch) in group {
        let depth = path
            .iter()
            .zip(previous)
            .take_while(|(a, b)| a == b)
            .count();
        let before = if previous.len() == parent.len() {
            first_count
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
        escape::append(&mut output, &patch.bytes, message.options.max_bytes)?;
        previous = path;
    }
    Ok(output)
}
