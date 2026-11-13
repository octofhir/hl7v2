use super::{FieldSchema, Kind, Primitive, Ref, definition, identifier, take};
use crate::{
    schema::Error,
    wire::{Date, DateTime, Decimal, Presence},
};
use std::borrow::Cow;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Requirement {
    Required,
    Forbidden,
}
#[derive(Clone, Copy)]
pub enum Predicate<'a> {
    Is { field: usize, state: Presence },
    Equals { field: usize, value: &'a str },
}
/// Additive presence constraint on an optional field in one named segment profile.
#[derive(Clone, Copy)]
pub struct Condition<'a> {
    pub profile: &'a str,
    pub target: usize,
    pub when: Predicate<'a>,
    pub requirement: Requirement,
}
#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Test<'a> {
    Is(u8),
    Equals(Cow<'a, str>),
}
pub(super) struct CompiledCondition<'a> {
    pub target: usize,
    pub selector: usize,
    pub test: Test<'a>,
    pub requirement: Requirement,
}
pub(super) fn presence_tag(state: Presence) -> u8 {
    match state {
        Presence::Missing => 0,
        Presence::Empty => 1,
        Presence::Null => 2,
        Presence::Value => 3,
    }
}
pub(super) fn compile<'a>(
    schema: &mut FieldSchema<'a>,
    definitions: &'a [Condition<'a>],
    budget: &mut usize,
    work: &mut usize,
) -> Result<(), Error> {
    take(budget, definitions.len())?;
    take(work, definitions.len())?;
    for rule in definitions {
        if !identifier(rule.profile) {
            return Err(definition());
        }
        let profile = schema
            .segments
            .binary_search_by(|s| s.name.as_ref().cmp(rule.profile))
            .map_err(|_| definition())?;
        let segment = &schema.segments[profile];
        let target = segment
            .fields
            .binary_search_by_key(&rule.target, |s| s.position)
            .map_err(|_| definition())?;
        let target = &segment.fields[target];
        if target.occurs.min != 0
            || (rule.requirement == Requirement::Required && target.occurs.max == Some(0))
        {
            return Err(definition());
        }
        let selector = match rule.when {
            Predicate::Is { field, .. } | Predicate::Equals { field, .. } => field,
        };
        if selector == rule.target {
            return Err(definition());
        }
        let source = segment
            .fields
            .binary_search_by_key(&selector, |s| s.position)
            .map_err(|_| definition())?;
        let source = &segment.fields[source];
        let test = match rule.when {
            Predicate::Is { state, .. } => Test::Is(presence_tag(state)),
            Predicate::Equals { value, .. } => {
                if value.len() > 4096 {
                    return Err(super::Error::new(super::ErrorKind::Limit));
                }
                take(work, value.len())?;
                if value.is_empty()
                    || value.chars().any(char::is_control)
                    || source.occurs.max != Some(1)
                {
                    return Err(definition());
                }
                let Ref::Fixed(index) = source.datatype else {
                    return Err(definition());
                };
                let valid = match &schema.types[index].kind {
                    Kind::Primitive(Primitive::Bytes | Primitive::Utf8) => true,
                    Kind::Primitive(Primitive::Ascii) => value.is_ascii(),
                    Kind::Primitive(Primitive::Number) => Decimal::parse(value.as_bytes()).is_ok(),
                    Kind::Primitive(Primitive::Date) => Date::parse(value.as_bytes()).is_ok(),
                    Kind::Primitive(Primitive::DateTime) => {
                        DateTime::parse(value.as_bytes()).is_ok()
                    }
                    Kind::Codes(values) => {
                        values.binary_search_by(|s| s.as_ref().cmp(value)).is_ok()
                    }
                    _ => false,
                };
                if !valid {
                    return Err(definition());
                }
                Test::Equals(Cow::Borrowed(value))
            }
        };
        schema.segments[profile].conditions.push(CompiledCondition {
            target: rule.target,
            selector,
            test,
            requirement: rule.requirement,
        });
    }
    for segment in &mut schema.segments {
        segment.conditions.sort_unstable_by(|a, b| {
            (a.target, a.selector, &a.test, a.requirement).cmp(&(
                b.target,
                b.selector,
                &b.test,
                b.requirement,
            ))
        });
        // Identical premises cannot duplicate a rule or require and forbid its target simultaneously.
        if segment.conditions.windows(2).any(|p| {
            (p[0].target, p[0].selector, &p[0].test) == (p[1].target, p[1].selector, &p[1].test)
        }) {
            return Err(definition());
        }
    }
    Ok(())
}
#[cfg(feature = "schema-package")]
impl CompiledCondition<'_> {
    pub(super) fn into_owned(self) -> CompiledCondition<'static> {
        CompiledCondition {
            target: self.target,
            selector: self.selector,
            requirement: self.requirement,
            test: match self.test {
                Test::Is(state) => Test::Is(state),
                Test::Equals(s) => Test::Equals(Cow::Owned(s.into_owned())),
            },
        }
    }
    pub(super) fn identity(&self, hash: &mut crate::schema::package::Identity) {
        hash.number(self.target);
        hash.number(self.selector);
        match &self.test {
            Test::Is(state) => {
                hash.tag(0);
                hash.tag(*state);
            }
            Test::Equals(s) => {
                hash.tag(1);
                hash.text(s);
            }
        }
        hash.tag(match self.requirement {
            Requirement::Required => 0,
            Requirement::Forbidden => 1,
        });
    }
    pub(super) fn write_file(
        &self,
        w: &mut crate::schema::package::file::Writer,
    ) -> Result<(), crate::schema::package::FileError> {
        w.count(self.target)?;
        w.count(self.selector)?;
        match &self.test {
            Test::Is(state) => {
                w.tag(0)?;
                w.tag(*state)?;
            }
            Test::Equals(s) => {
                w.tag(1)?;
                w.text(s)?;
            }
        }
        w.tag(match self.requirement {
            Requirement::Required => 0,
            Requirement::Forbidden => 1,
        })
    }
}
