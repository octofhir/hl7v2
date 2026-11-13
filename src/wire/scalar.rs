//! Explicit lexical views. No arithmetic, timezone inference or schema length validation.
use super::{Error, ErrorKind, Message, Presence, Span, Value, valid_path};
use std::{borrow::Cow, cmp::Ordering};

/// Exact decimal spelling with explicit numeric queries; no arithmetic or implicit normalization.
#[derive(Clone)]
pub struct Decimal<'a> {
    bytes: Cow<'a, [u8]>,
    point: usize,
}
impl<'a> Decimal<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        Self::from_bytes(Cow::Borrowed(bytes), 0..bytes.len())
    }
    fn from_bytes(bytes: Cow<'a, [u8]>, span: Span) -> Result<Self, Error> {
        let start = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
        let mut point = bytes.len();
        let mut digit = false;
        for (i, &byte) in bytes.iter().enumerate().skip(start) {
            if byte.is_ascii_digit() {
                digit = true;
            } else if byte == b'.' && point == bytes.len() {
                point = i;
            } else {
                return Err(Error::new(ErrorKind::Number, span));
            }
        }
        if !digit {
            return Err(Error::new(ErrorKind::Number, span));
        }
        Ok(Self { bytes, point })
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    /// An explicit sign, including a leading plus and negative zero.
    pub fn sign(&self) -> Option<u8> {
        self.bytes
            .first()
            .copied()
            .filter(|b| matches!(b, b'+' | b'-'))
    }
    pub fn integer_digits(&self) -> &[u8] {
        &self.bytes[usize::from(self.sign().is_some())..self.point]
    }
    /// `None` means no point; `Some([])` retains a trailing point.
    pub fn fraction_digits(&self) -> Option<&[u8]> {
        (self.point < self.bytes.len()).then(|| &self.bytes[self.point + 1..])
    }
    pub fn scale(&self) -> usize {
        self.fraction_digits().map_or(0, <[u8]>::len)
    }
    /// Numeric zero, including signed zero and any number of zero digits.
    pub fn is_zero(&self) -> bool {
        self.integer_digits()
            .iter()
            .chain(self.fraction_digits().unwrap_or(&[]))
            .all(|&b| b == b'0')
    }
    /// Compare mathematical values without allocating, padding or modifying their lexical bytes.
    /// Leading integer zeros, trailing fractional zeros and the sign of zero do not affect order.
    pub fn numeric_cmp(&self, other: &Decimal<'_>) -> Ordering {
        let negative = self.sign() == Some(b'-');
        let other_negative = other.sign() == Some(b'-');
        if negative != other_negative {
            return if self.is_zero() && other.is_zero() {
                Ordering::Equal
            } else if negative {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        let left = self.significant_integer_digits();
        let right = other.significant_integer_digits();
        let mut order = left.len().cmp(&right.len()).then_with(|| left.cmp(right));
        if order == Ordering::Equal {
            let left = self.fraction_digits().unwrap_or(&[]);
            let right = other.fraction_digits().unwrap_or(&[]);
            let shared = left.len().min(right.len());
            order = left[..shared].cmp(&right[..shared]).then_with(|| {
                // At least one tail is empty; the other is compared against virtual zero padding.
                left[shared..]
                    .iter()
                    .any(|&b| b != b'0')
                    .cmp(&right[shared..].iter().any(|&b| b != b'0'))
            });
        }
        if negative { order.reverse() } else { order }
    }
    /// Exact representability with `precision` total digits and `scale` fractional places.
    /// Requires 1 <= precision and scale <= precision. No rounding or rescaling is performed.
    /// Leading integer zeros are ignored; fractional digits beyond scale must all be zero.
    pub fn fits_precision_scale(&self, precision: usize, scale: usize) -> bool {
        precision > 0
            && scale <= precision
            && self.significant_integer_digits().len() <= precision - scale
            && self
                .fraction_digits()
                .unwrap_or(&[])
                .get(scale..)
                .is_none_or(|tail| tail.iter().all(|&b| b == b'0'))
    }
    fn significant_integer_digits(&self) -> &[u8] {
        let digits = self.integer_digits();
        let start = digits
            .iter()
            .position(|&b| b != b'0')
            .unwrap_or(digits.len());
        &digits[start..]
    }
}

/// Populated precision, without filling in missing calendar or clock components.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Precision {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    Fraction(u8),
}

/// DT lexical view: year, year/month or full Gregorian date.
#[derive(Clone)]
pub struct Date<'a>(DateTime<'a>);
impl<'a> Date<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        Self::from_bytes(Cow::Borrowed(bytes), 0..bytes.len())
    }
    fn from_bytes(bytes: Cow<'a, [u8]>, span: Span) -> Result<Self, Error> {
        DateTime::from_bytes(bytes, span, true).map(Self)
    }
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
    pub fn precision(&self) -> Precision {
        self.0.precision()
    }
    pub fn year(&self) -> u16 {
        self.0.year()
    }
    pub fn month(&self) -> Option<u8> {
        self.0.month()
    }
    pub fn day(&self) -> Option<u8> {
        self.0.day()
    }
}

/// DTM lexical view; an absent offset stays absent, and fractional zeros are retained.
#[derive(Clone)]
pub struct DateTime<'a> {
    bytes: Cow<'a, [u8]>,
    parts: [u16; 6],
    precision: Precision,
    offset: Option<i16>,
}
impl<'a> DateTime<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        Self::from_bytes(Cow::Borrowed(bytes), 0..bytes.len(), false)
    }
    fn from_bytes(bytes: Cow<'a, [u8]>, span: Span, date_only: bool) -> Result<Self, Error> {
        let kind = if date_only {
            ErrorKind::Date
        } else {
            ErrorKind::DateTime
        };
        let invalid = || Error::new(kind, span.clone());
        if bytes.len() > if date_only { 8 } else { 24 } {
            return Err(invalid());
        }
        let zone = if bytes.len() >= 5 && matches!(bytes[bytes.len() - 5], b'+' | b'-') {
            bytes.len() - 5
        } else {
            bytes.len()
        };
        let body = &bytes[..zone];
        let point = body.iter().position(|&b| b == b'.').unwrap_or(body.len());
        let mut precision = match point {
            4 => Precision::Year,
            6 => Precision::Month,
            8 => Precision::Day,
            10 if !date_only => Precision::Hour,
            12 if !date_only => Precision::Minute,
            14 if !date_only => Precision::Second,
            _ => return Err(invalid()),
        };
        if !body[..point].iter().all(u8::is_ascii_digit) {
            return Err(invalid());
        }
        if point < body.len() {
            let fraction = &body[point + 1..];
            if date_only
                || point != 14
                || !(1..=4).contains(&fraction.len())
                || !fraction.iter().all(u8::is_ascii_digit)
            {
                return Err(invalid());
            }
            precision = Precision::Fraction(fraction.len() as u8);
        }
        let mut parts = [0; 6];
        parts[0] = number(&body[..4]);
        for (slot, pair) in parts[1..].iter_mut().zip(body[4..point].as_chunks::<2>().0) {
            *slot = number(pair);
        }
        let [year, month, day, hour, minute, second] = parts;
        let leap =
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
        let days = match month {
            2 if leap => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        if year == 0
            || (point >= 6 && !(1..=12).contains(&month))
            || (point >= 8 && !(1..=days).contains(&day))
            || hour > 23
            || minute > 59
            || second > 59
        {
            return Err(invalid());
        }
        let offset = if zone < bytes.len() {
            let suffix = &bytes[zone..];
            if date_only || !suffix[1..].iter().all(u8::is_ascii_digit) {
                return Err(invalid());
            }
            let hour = number(&suffix[1..3]);
            let minute = number(&suffix[3..5]);
            if hour > 23 || minute > 59 {
                return Err(invalid());
            }
            let minutes = (hour * 60 + minute) as i16;
            Some(if suffix[0] == b'-' { -minutes } else { minutes })
        } else {
            None
        };
        Ok(Self {
            bytes,
            parts,
            precision,
            offset,
        })
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn precision(&self) -> Precision {
        self.precision
    }
    pub fn year(&self) -> u16 {
        self.parts[0]
    }
    pub fn month(&self) -> Option<u8> {
        (self.precision >= Precision::Month).then_some(self.parts[1] as u8)
    }
    pub fn day(&self) -> Option<u8> {
        (self.precision >= Precision::Day).then_some(self.parts[2] as u8)
    }
    pub fn hour(&self) -> Option<u8> {
        (self.precision >= Precision::Hour).then_some(self.parts[3] as u8)
    }
    pub fn minute(&self) -> Option<u8> {
        (self.precision >= Precision::Minute).then_some(self.parts[4] as u8)
    }
    pub fn second(&self) -> Option<u8> {
        (self.precision >= Precision::Second).then_some(self.parts[5] as u8)
    }
    pub fn fraction_digits(&self) -> Option<&[u8]> {
        match self.precision {
            Precision::Fraction(n) => Some(&self.bytes[15..15 + usize::from(n)]),
            _ => None,
        }
    }
    /// Signed minutes east of UTC; this does not infer the sender's local zone.
    pub fn offset_minutes(&self) -> Option<i16> {
        self.offset
    }
}
// Called only for validated ASCII digits of at most four characters.
fn number(bytes: &[u8]) -> u16 {
    bytes.iter().fold(0, |n, b| n * 10 + u16::from(b - b'0'))
}

/// TS: encoded DTM plus an optional legacy precision declaration.
#[derive(Clone)]
pub struct Timestamp<'a> {
    source: Value<'a>,
    time: DateTime<'a>,
    declared: Option<Precision>,
    presence: Presence,
}
impl<'a> Timestamp<'a> {
    pub fn source(&self) -> Value<'a> {
        self.source
    }
    /// Populated DTM components; use `precision()` to interpret their declared significance.
    pub fn encoded_time(&self) -> &DateTime<'a> {
        &self.time
    }
    pub fn precision(&self) -> Precision {
        self.declared.unwrap_or(self.time.precision())
    }
    pub fn declared_precision(&self) -> Option<Precision> {
        self.declared
    }
    pub fn precision_presence(&self) -> Presence {
        self.presence
    }
}

impl Message {
    /// Read one TS field, repetition or embedded component. Repetitions must be selected explicitly.
    pub fn timestamp(&self, path: &[usize]) -> Result<Timestamp<'_>, Error> {
        if !valid_path(path) || path.len() > 4 {
            return Err(Error::new(ErrorKind::Path, 0..0));
        }
        let source = self.value(path);
        let span = source.span().unwrap_or(0..0);
        match source.presence() {
            Presence::Missing => return Err(Error::new(ErrorKind::Missing, span)),
            Presence::Empty => return Err(Error::new(ErrorKind::Empty, span)),
            Presence::Null => return Err(Error::new(ErrorKind::Null, span)),
            Presence::Value => {}
        }
        let components = if path.len() == 2 {
            if source.child_count() != 1 {
                return Err(Error::new(ErrorKind::Composite, span));
            }
            source.child(1)
        } else {
            source
        };
        if !(1..=2).contains(&components.child_count()) {
            return Err(Error::new(ErrorKind::Composite, span));
        }
        let time = components.child(1).date_time()?;
        let declaration = components.child(2);
        let presence = declaration.presence();
        let declared = match presence {
            Presence::Missing | Presence::Empty => None,
            _ => {
                let bytes = declaration.decode()?;
                let precision = match bytes.as_ref() {
                    b"Y" => Precision::Year,
                    b"L" => Precision::Month,
                    b"D" => Precision::Day,
                    b"H" => Precision::Hour,
                    b"M" => Precision::Minute,
                    b"S" => Precision::Second,
                    _ => {
                        return Err(Error::new(
                            ErrorKind::Timestamp,
                            declaration.span().unwrap_or(0..0),
                        ));
                    }
                };
                if precision > time.precision() {
                    return Err(Error::new(
                        ErrorKind::Timestamp,
                        declaration.span().unwrap_or(0..0),
                    ));
                }
                Some(precision)
            }
        };
        Ok(Timestamp {
            source,
            time,
            declared,
            presence,
        })
    }
}

impl<'a> Value<'a> {
    fn typed_bytes(self) -> Result<(Cow<'a, [u8]>, Span), Error> {
        let bytes = self.decode()?;
        let span = self.span().unwrap_or(0..0);
        if bytes.is_empty() {
            return Err(Error::new(ErrorKind::Empty, span));
        }
        Ok((bytes, span))
    }
    /// Explicit numeric interpretation; the wire parser never chooses a datatype.
    pub fn decimal(self) -> Result<Decimal<'a>, Error> {
        let (bytes, span) = self.typed_bytes()?;
        Decimal::from_bytes(bytes, span)
    }
    pub fn date(self) -> Result<Date<'a>, Error> {
        let (bytes, span) = self.typed_bytes()?;
        Date::from_bytes(bytes, span)
    }
    pub fn date_time(self) -> Result<DateTime<'a>, Error> {
        let (bytes, span) = self.typed_bytes()?;
        DateTime::from_bytes(bytes, span, false)
    }
}
