//! Golden byte snapshots for every `Message` variant.
//!
//! These lock the wire format: any accidental change in field names,
//! enum variants, or ciborium behavior will fail these tests. Update
//! deliberately via `cargo insta review` when the change is intentional.

mod fixtures;

use std::fmt::Write;

#[test]
fn canonical_messages_wire_format() {
    let mut out = String::new();
    for (name, message) in fixtures::canonical_messages() {
        let mut bytes = Vec::<u8>::new();
        ciborium::into_writer(&message, &mut bytes).expect("encode");
        writeln!(out, "{name}:").unwrap();
        writeln!(out, "  {} bytes", bytes.len()).unwrap();
        writeln!(out, "  {}", hex(&bytes)).unwrap();
    }
    insta::assert_snapshot!("canonical_messages", out);
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{b:02x}").unwrap();
    }
    s
}
