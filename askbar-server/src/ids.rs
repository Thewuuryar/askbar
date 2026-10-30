//! Server-generated ids: ULIDs, which sort by creation time.

use std::sync::Mutex;
use ulid::Generator;

static GENERATOR: Mutex<Option<Generator>> = Mutex::new(None);

/// A new ULID as its 26-character string. Ids made by one process are strictly
/// increasing, even within the same millisecond, so they sort in creation order.
pub fn new_id() -> String {
    let mut guard = GENERATOR.lock().unwrap_or_else(|e| e.into_inner());
    let gen = guard.get_or_insert_with(Generator::new);
    // `generate` fails only if the random part overflows within one millisecond.
    gen.generate().unwrap_or_else(|_| ulid::Ulid::new()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_valid_ulids_that_sort_in_creation_order() {
        let ids: Vec<String> = (0..2000).map(|_| new_id()).collect();
        for id in &ids {
            assert_eq!(id.len(), 26, "{id}");
            assert!(ulid::Ulid::from_string(id).is_ok(), "{id}");
        }
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "ids must be strictly increasing");
    }
}
