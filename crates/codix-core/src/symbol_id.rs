//! Symbol IDs: a readable path plus a short disambiguating hash, such as
//! `py:src/billing/invoice.py::Invoice.total#a3f9`.

/// Hex digits of the hash.
const HASH_DIGITS: usize = 4;

/// The ID of a symbol. The hash comes from the kind and the signature, so
/// overloads differ while edits to the body or elsewhere leave the ID alone.
pub fn format(
    prefix: &str,
    path: &str,
    qualified_name: &str,
    kind: &str,
    signature: Option<&str>,
) -> String {
    format!(
        "{prefix}:{path}::{qualified_name}#{}",
        hash(kind, signature)
    )
}

fn hash(kind: &str, signature: Option<&str>) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(kind.as_bytes());
    hasher.update(&[0]);
    hasher.update(signature.unwrap_or_default().as_bytes());
    hasher.finalize().to_hex()[..HASH_DIGITS].to_owned()
}
