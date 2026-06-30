//! Map helpers with the same semantics the analyzer expects.

use std::collections::HashMap;
use std::hash::Hash;

/// Returns the value for `key`, panicking when it is missing.
pub(crate) fn must_get<'a, K, V>(map: &'a HashMap<K, V>, key: &K) -> &'a V
where
    K: Eq + Hash,
{
    match map.get(key) {
        Some(value) => value,
        None => panic!("Internal error: map missing key"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn must_get_returns_value() {
        let mut map: HashMap<&str, i32> = HashMap::new();
        map.insert("a", 1);
        assert_eq!(must_get(&map, &"a"), &1);
    }

    #[test]
    #[should_panic(expected = "Internal error: map missing key")]
    fn must_get_panics_on_missing() {
        let map: HashMap<&str, i32> = HashMap::new();
        must_get(&map, &"b");
    }
}
