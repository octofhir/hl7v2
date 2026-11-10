//! Inspect structure without printing clinical field content; optionally emit the original.
use octofhir_hl7v2::wire::{Message, ParseOptions};
use std::{
    fs::File,
    io::{Read, Write},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let path = arguments
        .next()
        .ok_or("usage: inspect FILE [--compatible] [--original]")?;
    let mut options = ParseOptions::default();
    let mut original = false;
    for argument in arguments {
        match argument.as_str() {
            "--compatible" => options.compatible_line_endings = true,
            "--original" => original = true,
            _ => return Err("usage: inspect FILE [--compatible] [--original]".into()),
        }
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(options.max_bytes as u64 + 1)
        .read_to_end(&mut bytes)?;
    let message = Message::parse_with_options(bytes, options)?;
    if original {
        std::io::stdout().lock().write_all(message.original())?;
        return Ok(());
    }
    println!(
        "{} bytes, {} segments, {} nodes",
        message.original().len(),
        message.segments().len(),
        message.node_count()
    );
    for segment in message.segments() {
        println!(
            "{} #{} {:?}: {} fields",
            std::str::from_utf8(segment.id())?,
            segment.occurrence(),
            segment.span(),
            segment.field_count()
        );
        for number in 1..=segment.field_count() {
            let field = segment.field(number);
            println!(
                "  {number}: {:?}, {:?}, {} repetitions",
                field.presence(),
                field.span(),
                field.child_count()
            );
        }
    }
    for diagnostic in message.diagnostics() {
        println!("diagnostic: {diagnostic:?}");
    }
    if message.omitted_diagnostics() > 0 {
        println!("{} diagnostics omitted", message.omitted_diagnostics());
    }
    Ok(())
}
