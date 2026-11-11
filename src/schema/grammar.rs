use super::{Content, Error, ErrorKind, MatchLimits, Occurs, Rule, Structure, identifier};
use crate::wire::{Message, Segment};
use std::ops::Range;

struct CompiledRule<'a> {
    name: &'a str,
    occurs: Occurs,
    segment: Option<[u8; 3]>,
    profile: Option<&'a str>,
    children: Range<usize>,
    first: Vec<[u8; 3]>,
}
/// An immutable deterministic sequence/group grammar, reusable across messages.
pub struct StructureSchema<'a> {
    rules: Vec<CompiledRule<'a>>,
}
impl StructureSchema<'_> {
    #[cfg(feature = "schema-package")]
    pub(super) fn identity(&self, hash: &mut super::package::Identity) {
        hash.number(self.rules.len());
        for rule in &self.rules {
            hash.text(rule.name);
            hash.occurs(rule.occurs);
            match rule.segment {
                Some(id) => {
                    hash.tag(1);
                    hash.bytes(&id);
                    match rule.profile {
                        Some(profile) => {
                            hash.tag(1);
                            hash.text(profile);
                        }
                        None => hash.tag(0),
                    }
                }
                None => {
                    hash.tag(0);
                    hash.number(rule.children.start);
                    hash.number(rule.children.end);
                }
            }
        }
    }
    pub(super) fn profiles(&self) -> impl Iterator<Item = ([u8; 3], Option<&str>)> {
        self.rules
            .iter()
            .filter_map(|r| r.segment.map(|id| (id, r.profile)))
    }
    pub fn name(&self) -> &str {
        self.rules[0].name
    }
    /// Match segment shape only. Use `Catalog::group` to also resolve the envelope.
    pub fn group<'s, 'm>(
        &'s self,
        message: &'m Message,
        limits: MatchLimits,
    ) -> Result<View<'s, 'm>, Error> {
        let mut matcher = Matcher {
            schema: self,
            message,
            nodes: Vec::new(),
            cursor: 1,
            limits,
            steps: limits.max_steps,
        };
        matcher.apply(0)?;
        if matcher.cursor <= message.segments().len() {
            return Err(matcher.error(ErrorKind::UnexpectedSegment, 0));
        }
        Ok(View {
            schema: self,
            message,
            nodes: matcher.nodes,
        })
    }
}

pub(super) fn compile<'a>(
    definition: &'a Structure<'a>,
    depth: usize,
    rules: &mut usize,
    work: &mut usize,
) -> Result<StructureSchema<'a>, Error> {
    if definition.rules.first().is_none_or(|r| {
        !matches!(r.content, Content::Segment(id) | Content::TypedSegment { id, .. } if id == *b"MSH") || r.occurs != Occurs::ONCE
    }) {
        return Err(Error::new(ErrorKind::Definition));
    }
    take(rules, 1)?;
    let mut nodes = vec![CompiledRule {
        name: definition.name,
        occurs: Occurs::ONCE,
        segment: None,
        profile: None,
        children: 0..0,
        first: Vec::new(),
    }];
    fill(&mut nodes, 0, definition.rules, depth, rules, work)?;
    follow(&nodes, 0, &[], work)?;
    Ok(StructureSchema { rules: nodes })
}
fn take(budget: &mut usize, count: usize) -> Result<(), Error> {
    *budget = budget
        .checked_sub(count)
        .ok_or_else(|| Error::new(ErrorKind::Limit))?;
    Ok(())
}
fn union(a: &mut Vec<[u8; 3]>, b: &[[u8; 3]], work: &mut usize) -> Result<(), Error> {
    take(work, a.len().saturating_add(b.len()))?;
    a.extend_from_slice(b);
    a.sort_unstable();
    a.dedup();
    Ok(())
}
fn fill<'a>(
    nodes: &mut Vec<CompiledRule<'a>>,
    parent: usize,
    definitions: &'a [Rule<'a>],
    depth: usize,
    budget: &mut usize,
    work: &mut usize,
) -> Result<(), Error> {
    if depth == 0 {
        return Err(Error::new(ErrorKind::Limit).rule(parent));
    }
    if definitions.is_empty() {
        return Err(Error::new(ErrorKind::Definition).rule(parent));
    }
    take(budget, definitions.len())?;
    take(work, definitions.len())?;
    for (offset, rule) in definitions.iter().enumerate() {
        if !identifier(rule.name) {
            return Err(Error::new(ErrorKind::Definition).rule(nodes.len() + offset));
        }
    }
    let mut names: Vec<_> = definitions.iter().map(|r| r.name).collect();
    names.sort_unstable();
    if names.windows(2).any(|p| p[0] == p[1]) {
        return Err(Error::new(ErrorKind::Definition).rule(parent));
    }
    let start = nodes.len();
    for rule in definitions {
        let index = nodes.len();
        if rule
            .occurs
            .max
            .is_some_and(|max| max == 0 || max < rule.occurs.min)
        {
            return Err(Error::new(ErrorKind::Definition).rule(index));
        }
        let segment = match rule.content {
            Content::Segment(id) | Content::TypedSegment { id, .. } => {
                if !id
                    .iter()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
                    || (id == *b"MSH" && index != 1)
                {
                    return Err(Error::new(ErrorKind::Definition).rule(index));
                }
                Some(id)
            }
            Content::Group(_) => None,
        };
        let profile = match rule.content {
            Content::TypedSegment { profile, .. } => {
                if !identifier(profile) {
                    return Err(Error::new(ErrorKind::Definition).rule(index));
                }
                Some(profile)
            }
            _ => None,
        };
        nodes.push(CompiledRule {
            name: rule.name,
            occurs: rule.occurs,
            segment,
            profile,
            children: 0..0,
            first: segment.into_iter().collect(),
        });
    }
    let children = start..nodes.len();
    nodes[parent].children = children.clone();
    for (index, definition) in children.clone().zip(definitions) {
        if let Content::Group(children) = definition.content {
            fill(nodes, index, children, depth - 1, budget, work)?;
        }
    }
    let mut first = Vec::new();
    let mut nullable = true;
    for child in children {
        union(&mut first, &nodes[child].first, work)?;
        if nodes[child].occurs.min > 0 {
            nullable = false;
            break;
        }
    }
    if nullable {
        return Err(Error::new(ErrorKind::Definition).rule(parent));
    }
    nodes[parent].first = first;
    Ok(())
}
// A variable occurrence cannot share its FIRST set with anything that may follow it.
fn follow(
    nodes: &[CompiledRule<'_>],
    parent: usize,
    after: &[[u8; 3]],
    work: &mut usize,
) -> Result<(), Error> {
    let mut next = after.to_vec();
    for child in nodes[parent].children.clone().rev() {
        let rule = &nodes[child];
        take(work, rule.first.len().saturating_add(next.len()))?;
        if rule.occurs.max != Some(rule.occurs.min)
            && rule.first.iter().any(|id| next.binary_search(id).is_ok())
        {
            return Err(Error::new(ErrorKind::AmbiguousGrammar).rule(child));
        }
        if rule.segment.is_none() {
            let mut end = next.clone();
            if rule.occurs.max.is_none_or(|max| max > 1) {
                union(&mut end, &rule.first, work)?;
            }
            follow(nodes, child, &end, work)?;
        }
        if rule.occurs.min == 0 {
            union(&mut next, &rule.first, work)?;
        } else {
            next.clone_from(&rule.first);
        }
    }
    Ok(())
}

struct Node {
    rule: usize,
    segments: Range<usize>,
    end: usize,
}
struct Matcher<'s, 'm> {
    schema: &'s StructureSchema<'s>,
    message: &'m Message,
    nodes: Vec<Node>,
    cursor: usize,
    limits: MatchLimits,
    steps: usize,
}
impl Matcher<'_, '_> {
    fn error(&self, kind: ErrorKind, rule: usize) -> Error {
        let mut error = Error::new(kind).rule(rule);
        error.path[0] = self.cursor;
        let end = self.message.original().len();
        error.span = self
            .message
            .segment(self.cursor)
            .map_or(end..end, |s| s.span());
        error
    }
    fn apply(&mut self, index: usize) -> Result<(), Error> {
        let rule = &self.schema.rules[index];
        let mut count = 0;
        while rule.occurs.max.is_none_or(|max| count < max) {
            if self.steps == 0 {
                return Err(self.error(ErrorKind::Limit, index));
            }
            self.steps -= 1;
            if self
                .message
                .segment(self.cursor)
                .is_none_or(|s| rule.first.binary_search(s.id()).is_err())
            {
                if count < rule.occurs.min {
                    return Err(self.error(ErrorKind::MissingSegment, index));
                }
                break;
            }
            if self.nodes.len() >= self.limits.max_occurrences {
                return Err(self.error(ErrorKind::Limit, index));
            }
            let node = self.nodes.len();
            self.nodes.push(Node {
                rule: index,
                segments: self.cursor..self.cursor,
                end: 0,
            });
            if rule.segment.is_some() {
                self.cursor += 1;
            } else {
                for child in rule.children.clone() {
                    self.apply(child)?;
                }
            }
            self.nodes[node].segments.end = self.cursor;
            self.nodes[node].end = self.nodes.len();
            count += 1;
        }
        Ok(())
    }
}

/// Borrowed source plus a flat arena of group/segment occurrences; no owning message tree.
pub struct View<'s, 'm> {
    schema: &'s StructureSchema<'s>,
    message: &'m Message,
    nodes: Vec<Node>,
}
impl<'s, 'm> View<'s, 'm> {
    /// All segment leaves in source order, retaining their schema roles and field profiles.
    pub fn segments(&self) -> impl Iterator<Item = Occurrence<'_, 's, 'm>> {
        self.nodes.iter().enumerate().filter_map(|(index, node)| {
            self.schema.rules[node.rule]
                .segment
                .map(|_| Occurrence { view: self, index })
        })
    }
    pub fn message(&self) -> &'m Message {
        self.message
    }
    pub fn occurrence_count(&self) -> usize {
        self.nodes.len()
    }
    pub fn root(&self) -> Occurrence<'_, 's, 'm> {
        Occurrence {
            view: self,
            index: 0,
        }
    }
}
#[derive(Clone, Copy)]
pub struct Occurrence<'v, 's, 'm> {
    view: &'v View<'s, 'm>,
    index: usize,
}
impl<'v, 's, 'm> Occurrence<'v, 's, 'm> {
    pub fn profile(self) -> Option<&'s str> {
        self.view.schema.rules[self.view.nodes[self.index].rule].profile
    }
    pub fn name(self) -> &'s str {
        self.view.schema.rules[self.view.nodes[self.index].rule].name
    }
    /// Global, one-based, half-open source segment range.
    pub fn segment_range(self) -> Range<usize> {
        self.view.nodes[self.index].segments.clone()
    }
    pub fn segment(self) -> Option<Segment<'m>> {
        let node = &self.view.nodes[self.index];
        self.view.schema.rules[node.rule].segment?;
        self.view.message.segment(node.segments.start)
    }
    pub fn segments(self) -> impl Iterator<Item = Segment<'m>> {
        let range = self.segment_range();
        self.view
            .message
            .segments()
            .skip(range.start - 1)
            .take(range.len())
    }
    pub fn children(self) -> impl Iterator<Item = Self> + 'v {
        let mut next = self.index + 1;
        let end = self.view.nodes[self.index].end;
        std::iter::from_fn(move || {
            if next >= end {
                return None;
            }
            let index = next;
            next = self.view.nodes[index].end;
            Some(Self {
                view: self.view,
                index,
            })
        })
    }
}
