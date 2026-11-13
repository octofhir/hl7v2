# Sparse wire editing

An editor retains byte-range patches against its immutable source. `set` replaces an existing
position. `set_or_create` additionally extends fields, repetitions, components and subcomponents
inside an existing segment. All coordinates refer to the original revision. New non-MSH segments
can be built and inserted, and original non-MSH segments can be removed in the same editor.
Delimiters and MSH-1/2 cannot be edited; MSH itself cannot be inserted or removed.

```rust
use octofhir_hl7v2::wire::{Edit, Message, Presence};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let message = Message::parse(b"MSH|^~\\&|LAB\rZZZ|keep\r")?;
    let mut editor = message.edit();
    editor.set_or_create(&[2, 4, 2, 2], Edit::Value(b"literal|value".to_vec()))?;
    let revised = editor.finish()?;
    assert_eq!(revised.value(&[2, 4, 2, 2]).decode()?.as_ref(), b"literal|value");
    assert_eq!(revised.value(&[2, 2]).presence(), Presence::Empty);
    assert_eq!(revised.value(&[2, 3]).presence(), Presence::Empty);
    assert_eq!(message.value(&[2, 4]).presence(), Presence::Missing);
    assert_eq!(revised.value(&[2, 1]).decode()?.as_ref(), b"keep");
    Ok(())
}
```

## Positions and conflicts

Creating position N emits separators for every intervening sibling: those positions become
explicitly empty. ER7 cannot represent an absent interior sibling without shifting later values.
New parents have implicit first children. `Empty`, `Null` and literal quote bytes remain distinct;
`Value` accepts unescaped bytes and encodes them once using the message's separators.

Several creations sharing an existing parent are rendered together in path order, regardless of
call order. Replacing a pending operation at the same path keeps the last operation. A parent and
its descendant cannot both have pending edits. Expanding descendants of an explicit-null source
parent is refused; replace that parent in an explicit revision first. Untouched byte ranges,
unknown content and terminators are copied verbatim. Unterminated escapes block every mutation,
including when the diagnostic display budget hid them.

`Unset` removes only one originally final sibling per parent per revision. It cannot coexist with
creation of later siblings under that parent: doing so would shift their coordinates. Both call
orders fail with `EditConflict`. Unsetting an originally absent path cancels an exact pending
creation there, or is a no-op. It does not recursively cancel descendant edits; any sibling gaps
still needed by other creations remain empty.

## Creating a fresh message

`Message::create(delimiters, options)` creates a CR-terminated MSH containing only MSH-1 and
MSH-2. It validates the explicit separator declarations through the same wire parser and rejects
any declaration that would be reinterpreted as a different delimiter set. Input/index/segment
limits apply during creation. No release, event, control ID, timestamp or clinical defaults are
invented. Populate other header positions with `set_or_create` and add segment builders in one
editor transaction before resolving/validating the intended schema.

```rust
use octofhir_hl7v2::wire::{Delimiters, Message, ParseOptions, Presence};

let message = Message::create(
    Delimiters { field: b'|', encoding: *b"^~\\&", truncation: None },
    ParseOptions::default(),
)?;
assert_eq!(message.original(), b"MSH|^~\\&\r");
assert_eq!(message.value(&[1, 12]).presence(), Presence::Missing);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Segment construction and splicing

`Message::segment_builder(id)` creates a sparse `SegmentBuilder` using that message's delimiters
and limits. IDs follow the existing three-byte uppercase ASCII/digit rule; MSH is refused. Its
`set` paths are relative to the new segment: `[field, repetition, component, subcomponent]`, with
optional suffixes. Values use the same single-pass escaping as positional edits. `Empty`, `Null`
and literal quote bytes remain distinct. Gaps become empty siblings, including implicit first
children; assigning both an ancestor and its descendant conflicts. Reassigning an exact path
replaces it. `Unset` cancels only that exact draft assignment, retaining padding needed elsewhere.
A builder with no assignments emits a segment ID with no fields.

```rust
use octofhir_hl7v2::wire::{Edit, Message, Presence};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = Message::parse(b"MSH|^~\\&|LAB\rPID|synthetic-id\rNTE|obsolete\r")?;
    let mut obx = source.segment_builder(*b"OBX")?;
    obx.set(&[2], Edit::Value(b"NM".to_vec()))?;
    obx.set(&[5, 2], Edit::Value(b"+0001.2300".to_vec()))?;
    let mut editor = source.edit();
    editor.insert_segment(3, obx)?.remove_segment(3)?;
    let revised = editor.finish()?;
    assert_eq!(revised.segment(3).map(|s| *s.id()), Some(*b"OBX"));
    assert_eq!(revised.value(&[3, 5, 1]).presence(), Presence::Empty);
    assert_eq!(revised.value(&[3, 5, 2]).decimal()?.as_bytes(), b"+0001.2300");
    assert_eq!(source.segment(3).map(|s| *s.id()), Some(*b"NTE"));
    Ok(())
}
```

`Editor::insert_segment(before, builder)` inserts before an original one-based segment occurrence;
`N + 1` appends after an original N-segment message. Position 1 is protected. Multiple insertions
at the same anchor retain call order. The builder must originate from the editor's source message
or a clone sharing its index, preventing delimiter/limit mismatches across revisions. Insertion
consumes the builder and stores encoded bytes/node growth; it does not reparse a message.

`remove_segment(occurrence)` deletes the original segment and its own terminator. Repeated removal
is idempotent. Removing a segment with positional edits, or recording positional edits on a removed
segment, returns `EditConflict` in either order. Insertion anchors remain original coordinates even
when the anchored segment is removed. Inserted segments use their builder paths; edit their builder
before insertion or finish the revision before addressing their new global positions.

Every generated segment ends in CR. Original terminators, including compatible CRLF/LF, remain
unchanged. Appending after a retained original final segment without a terminator adds exactly one
boundary CR before the new segment. If that original segment is removed, no extra boundary is
added. Field-tail creation and append at the same original byte offset are ordered explicitly.
All structural mutations refuse unterminated source escapes, including with hidden diagnostics.
This is wire editing; no segment ordering, datatype or profile validity is inferred.

## Bounds and cost

Values are encoded and pending replacement bytes/count are bounded while recording operations.
Creation coordinates above `max_nodes` fail immediately. Serialization groups paths by their
existing parent, checks separator bytes and counts new index nodes before expanding gaps. A new
field requires four nodes, repetition three, component two and subcomponent one, including their
implicit first descendants. The node budget conservatively counts the original index plus new
field and inserted-segment nodes; deleting fields or segments grants no node capacity within the
revision. Full output bytes are bounded separately. Pending inserted segment bytes share the replacement-byte budget. Final segment count
is the original count minus removals plus insertions and is bounded by `max_segments`. Pending
insertion count is also capped independently before allocating metadata. Draft positions/assignment
counts and rendered node/separator growth are bounded before expanding gaps. A recorded operation
may therefore fail during serialization with `Limit`; a later whole-segment removal can make the
final byte/segment count fit, but cannot reclaim node capacity.

There is no full mutable message tree or reparse after each operation. Ordering/grouping cost is
O(P log P) for P patches; path depth is at most five. Rendering copies bounded source/replacement
bytes and emits each required separator once. Group buffers add at most one bounded copy of
created suffixes. Container overhead is additional. `serialize` returns bytes; `finish` also
reindexes with the original parser policy and limits. A failed operation or serialization leaves
the original unchanged. A fresh header declares delimiters only; schema validation remains an explicit separate pass.

The dependency-free wire benchmark also measures segment draft preparation, insertion/removal,
serialization and reindexing together against an already parsed source. Preflight checks verify
decoded new values, presence and retained segment bytes before timing. This boundary excludes
source parsing, schema validation, transport and durability; it is not a throughput guarantee.
