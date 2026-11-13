use super::{Edit, Group, Kind, Patch, render};
use crate::wire::{Error, ErrorKind, Message, escape};
use std::collections::BTreeMap;

/// Sparse unescaped values for a new non-MSH segment, using its source message's delimiters/limits.
/// Paths are [field, repetition, component, subcomponent], with optional suffixes.
pub struct SegmentBuilder<'a> {
    pub(super) message: &'a Message,
    id: [u8; 3],
    patches: BTreeMap<Vec<usize>, Patch>,
    bytes: usize,
}
impl<'a> SegmentBuilder<'a> {
    pub(crate) fn new(message: &'a Message, id: [u8; 3]) -> Result<Self, Error> {
        if id == *b"MSH" {
            return Err(Error::new(ErrorKind::Header, 0..0));
        }
        if !id
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            return Err(Error::new(ErrorKind::Segment, 0..0));
        }
        Ok(Self {
            message,
            id,
            patches: BTreeMap::new(),
            bytes: 0,
        })
    }
    /// Fill sparse positions with empty siblings. Unset cancels only the exact pending assignment.
    pub fn set(&mut self, path: &[usize], edit: Edit) -> Result<&mut Self, Error> {
        if !(1..=4).contains(&path.len()) || path.contains(&0) {
            return Err(Error::new(ErrorKind::Path, 0..0));
        }
        if path.iter().any(|&n| n > self.message.options.max_nodes) {
            return Err(Error::new(ErrorKind::Limit, 0..0));
        }
        // The shared sparse renderer uses a segment prefix; zero here is an internal draft ID.
        let mut key = Vec::with_capacity(path.len() + 1);
        key.push(0);
        key.extend_from_slice(path);
        if matches!(edit, Edit::Unset) {
            if let Some(old) = self.patches.remove(&key) {
                self.bytes -= old.bytes.len();
            }
            return Ok(self);
        }
        if (2..key.len()).any(|n| self.patches.contains_key(&key[..n]))
            || self
                .patches
                .range(key.clone()..)
                .next()
                .is_some_and(|(p, _)| p.len() > key.len() && p.starts_with(&key))
        {
            return Err(Error::new(ErrorKind::EditConflict, 0..0));
        }
        let max = self.message.options.max_bytes;
        let bytes = match edit {
            Edit::Value(value) => escape::encode(&value, self.message.delimiters(), max)?,
            Edit::Empty => Vec::new(),
            Edit::Null => b"\"\"".to_vec(),
            Edit::Unset => Vec::new(),
        };
        let old = self.patches.get(&key).map_or(0, |p| p.bytes.len());
        let remaining = self.bytes - old;
        if bytes.len() > max.saturating_sub(remaining)
            || (!self.patches.contains_key(&key)
                && self.patches.len() >= self.message.options.max_nodes)
        {
            return Err(Error::new(ErrorKind::Limit, 0..0));
        }
        self.bytes = remaining + bytes.len();
        self.patches.insert(
            key,
            Patch {
                span: 0..0,
                bytes,
                kind: Kind::Create(1),
            },
        );
        Ok(self)
    }
    pub(super) fn encode(&self) -> Result<(Vec<u8>, usize), Error> {
        let max = self.message.options.max_bytes;
        let mut budget = max
            .checked_sub(self.bytes)
            .and_then(|n| n.checked_sub(4))
            .ok_or_else(|| Error::new(ErrorKind::Limit, 0..0))?;
        let mut nodes = self.message.options.max_nodes;
        let group: Group<'_> = self
            .patches
            .iter()
            .map(|(p, v)| (p.as_slice(), v))
            .collect();
        let fields = render(&[0], &group, 0, self.message, &mut nodes, &mut budget)?;
        let mut bytes = Vec::new();
        escape::append(&mut bytes, &self.id, max)?;
        escape::append(&mut bytes, &fields, max)?;
        escape::append(&mut bytes, b"\r", max)?;
        Ok((bytes, self.message.options.max_nodes - nodes))
    }
}
