//! Persistence. On-disk schema structs are kept separate from the domain
//! model (explicit from_disk/to_disk mapping) so schema evolution never
//! contorts the domain. All writes are atomic: same-dir temp file + fsync +
//! rename + parent-dir fsync.

/// Current .omanote schema version.
pub const NOTE_SCHEMA: u32 = 1;
