# Sparse wire editing

An editor retains byte-range patches against its immutable source. `set` replaces an existing
position. `set_or_create` additionally extends fields, repetitions, components and subcomponents
inside an existing segment. All coordinates refer to the original revision. Delimiters, segment
creation and segment order are outside this API; MSH-1/2 cannot be edited.

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

## Bounds and cost

Values are encoded and pending replacement bytes/count are bounded while recording operations.
Creation coordinates above `max_nodes` fail immediately. Serialization groups paths by their
existing parent, checks separator bytes and counts new index nodes before expanding gaps. A new
field requires four nodes, repetition three, component two and subcomponent one, including their
implicit first descendants. The node budget conservatively counts the original index plus new
nodes; deletions do not grant extra capacity within the revision. Full output bytes are bounded
separately. A recorded creation may therefore fail during serialization with `Limit`.

There is no full mutable message tree or reparse after each operation. Ordering/grouping cost is
O(P log P) for P patches; path depth is at most five. Rendering copies bounded source/replacement
bytes and emits each required separator once. Group buffers add at most one bounded copy of
created suffixes. Container overhead is additional. `serialize` returns bytes; `finish` also
reindexes with the original parser policy and limits. A failed operation or serialization leaves
the original unchanged. This is not schema validation or a general outbound message builder.
