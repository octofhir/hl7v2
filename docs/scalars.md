# Exact scalar views

`wire::{Decimal, Date, DateTime, Timestamp}` validate explicit numeric/date interpretations while retaining
their exact lexical bytes. The generic parser does not choose datatypes or validate a schema.
Direct `parse(&[u8])` borrows its input. `Value::{decimal, date, date_time}` use the existing
single-pass scalar decoder: values without escapes borrow the original; escaped values own the
decoded bytes. Neither path uses floating point, JSON, a date library or implicit normalization.

```rust
use octofhir_hl7v2::wire::{Message, Precision};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let message = Message::parse(
        b"MSH|^~\\&|LAB\rZEX|+00012.3400|202402|20240229235959.0100-0000\r",
    )?;
    let number = message.value(&[2, 1]).decimal()?;
    assert_eq!(number.integer_digits(), b"00012");
    assert_eq!(number.fraction_digits(), Some(b"3400".as_slice()));
    let date = message.value(&[2, 2]).date()?;
    assert_eq!(date.precision(), Precision::Month);
    assert_eq!(date.day(), None);
    let timestamp = message.value(&[2, 3]).date_time()?;
    assert_eq!(timestamp.precision(), Precision::Fraction(4));
    assert_eq!(timestamp.offset_minutes(), Some(0));
    assert_eq!(timestamp.as_bytes(), b"20240229235959.0100-0000");
    Ok(())
}
```

## Supported lexical subset

- `Decimal` accepts an optional leading sign, digits and at most one decimal point, with at least
  one digit overall. `.5` and `5.` are accepted. Its digit slices retain zeros; `sign()` retains
  an explicit plus or minus, including negative zero. Exponents, whitespace, grouping separators,
  NaN/infinity and comparators are refused. There is no fixed coefficient width or arithmetic API.
  Numeric queries below are explicit. The wire message limit bounds ordinary input; standalone callers bound
  their own slices. A successful lexical parse does not enforce the v2.5.1 NM length of 16.
- `Date` accepts 4, 6 or 8 digits: year, month and day precision. Supplied components must form a
  valid Gregorian date in years 0001–9999. Missing components remain `None`.
- `DateTime` extends that form through hour, minute and second precision. A fraction requires
  seconds and contains 1–4 digits, retained as a slice. An optional signed four-digit HHMM offset
  is retained at every supported precision. Offset syntax is bounded to hours 00–23/minutes 00–59;
  checking legally applicable sender zones belongs to the interface. Clock hours are 00–23 and
  minutes/seconds 00–59; leap seconds and `24:00` are unsupported. No offset is inferred from the
  receiver, operating system or current date. `+0000` and `-0000` both yield zero minutes while
  remaining distinct spellings in `as_bytes()`.

These rules were checked against the v2.5.1 [datatype chapter](https://hl7.eu/HL7v2x/v251/std251/ch02a.html),
sections 2.A.1.21, 2.A.1.22, 2.A.1.47 and 2.A.1.77. This is lexical interpretation, not full schema conformance.
TM, arithmetic, FHIR emission and exact precision through FML remain separate work.

## Exact decimal queries

`Decimal::numeric_cmp(&other)` returns `std::cmp::Ordering` for the mathematical values. Leading
integer zeros, trailing fractional zeros and the sign of zero do not change numeric order.
`is_zero()` recognizes all valid zero spellings, including `-0.`, `+.0` and `000.000`.
`as_bytes()`, `sign()`, digit slices and lexical `scale()` remain unchanged. Numeric comparison is
explicit; no `Eq`/`Ord` traits or lexical normalization are added. Wire identity continues to use
original bytes.

`fits_precision_scale(precision, scale)` checks exact representability in a fixed-decimal target
with `precision` total digit positions and `scale` fractional places. It requires positive precision
and `scale <= precision`; invalid target configurations return false. At most `precision - scale`
integer digits may remain after ignoring leading zeros. Every fractional digit beyond the target
scale must be zero. A shorter fraction may be extended with virtual zeros. A target with equal
precision and scale can hold magnitudes below one, including zero.

```rust
use octofhir_hl7v2::wire::Decimal;
use std::cmp::Ordering;

let source = b"+0001.2300";
let value = Decimal::parse(source)?;
assert_eq!(value.numeric_cmp(&Decimal::parse(b"1.23")?), Ordering::Equal);
assert!(value.fits_precision_scale(3, 2));
assert!(!value.fits_precision_scale(3, 1));
assert!(Decimal::parse(b"-0.000")?.is_zero());
assert_eq!(value.scale(), 4);
assert_eq!(value.as_bytes(), source);
# Ok::<(), Box<dyn std::error::Error>>(())
```

These queries use linear scans over actual digit slices and constant auxiliary space. They allocate
no padded buffers, use no floating point and perform no rounding or rescaling. Even large target
parameters do not cause work proportional to the requested scale. Standalone callers bound their
input slices; wire-decoded views retain the existing message limits and original source references.

Precision/scale queries are opt-in caller policy, separate from schema length/lexical checks.
Registering an NM datatype does not activate them. The existing package identity/validation
semantics remain unchanged; hosts using these queries in processing decisions include the policy
and parameters in their complete processing plan. FHIR emission and exact numeric interoperability
through FML remain separate work.

## Composite TS

`Message::timestamp(path)` reads a single TS field (two coordinates), a selected repetition
(three) or an embedded component using subcomponents (four). A repeated field requires an explicit
repetition path. Extra components and a composite precision code fail instead of being ignored.
The result retains `source()` with raw bytes/spans and `encoded_time()` with the full DTM spelling.
Its `precision()` applies the optional legacy Y/L/D/H/M/S declaration, which may reduce but never
increase the populated precision. `declared_precision()` and `precision_presence()` distinguish
absent, empty and populated declarations; explicit null and unknown codes fail. A caller interpreting
TS must use the effective precision rather than inferring significance from `encoded_time()`.

```rust
use octofhir_hl7v2::wire::{Message, Precision};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let message = Message::parse(b"MSH|^~\\&|LAB\rZEX|202402291234^L\r")?;
    let timestamp = message.timestamp(&[2, 1])?;
    assert_eq!(timestamp.precision(), Precision::Month);
    assert_eq!(timestamp.encoded_time().as_bytes(), b"202402291234");
    Ok(())
}
```

## Errors and ownership

Typed wire reads distinguish `Missing`, `Empty`, `Null`, `Composite`, escape failures and invalid
`Number`/`Date`/`DateTime`/`Timestamp`. Explicit null is determined before decoding; escaped literal quotes
are populated data and fail numeric interpretation. All four presence states remain inspectable.
Datatype errors span the entire selected original value, including its escape spellings;
standalone errors span `0..input.len()`. Decoder errors retain their specific wire ranges.
Error messages do not include source values. Direct empty input fails datatype syntax; the
`Empty` error specifically identifies an empty wire position.

Typed views cannot outlive borrowed input. Decoded buffers are retained by their returned view;
there is no intermediate `String` or second byte copy. `clone()` shares borrowed slices but copies
an owned decoded buffer. The message, source spans, diagnostics and original serialization never
change. Parsing is linear for decimals and bounded to 8/24 decoded bytes for DT/DTM; escape decoding
retains the message's existing limits. These views do not attach diagnostics to an immutable message.

Five lexical integration scenarios cover long decimals, grammar rejection, precision/calendar
boundaries, offsets, presence, source spans and custom delimiters. The wire fuzz target exercises
standalone arbitrary bytes, typed reads from parsed nodes and TS paths at all supported depths.
Three numeric-query scenarios additionally cover ordered equivalence classes, signed zero,
precision/scale bounds, independent scaled-integer oracles, 100,000-digit inputs and escaped wire
ownership. Fuzzing checks numeric order/representability against bounded i128 arithmetic and verifies
invariance under added insignificant zeros. The local benchmark reports lexical parse timing and
queries on already parsed decimal views separately from message parsing. These measurements exclude
schema and FHIR validation.
