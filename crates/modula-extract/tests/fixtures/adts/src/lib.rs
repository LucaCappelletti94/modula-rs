//! Fixture exercising enum and union extraction and their field signature edges
//! to local types.

/// A local type carried by enum variants and a union field. `Copy` so it can be
/// a union field directly.
#[derive(Clone, Copy)]
pub struct Payload;

/// A second local type, carried by a struct-like variant.
#[derive(Clone, Copy)]
pub struct Header;

/// An enum whose variants reference local types: `Message -> Payload` (tuple
/// variant and the struct variant body) and `Message -> Header` (struct variant).
pub enum Message {
    Empty,
    Data(Payload),
    Framed { header: Header, body: Payload },
}

/// A union with a field referencing a local type: `Raw -> Payload`.
pub union Raw {
    pub payload: Payload,
    pub bits: u64,
}
