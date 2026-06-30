//! Map helpers with the same semantics the analyzer expects.

use std::collections::HashMap;
use std::hash::Hash;

/// Returns the value for `key`, inserting a freshly built default when absent.
pub(crate) fn get_or_create<K, V, F>(input: &mut HashMap<K, V>, key: K, build_default: F) -> V
where
    K: Eq + Hash + Clone,
    V: Clone,
    F: FnOnce() -> V,
{
    if let Some(value) = input.get(&key) {
        return value.clone();
    }
    let default_value = build_default();
    input.insert(key, default_value.clone());
    default_value
}

/// Returns `true` when both maps have the same keys mapped to the same values.
pub(crate) fn are_maps_equal<K, V>(a: &HashMap<K, V>, b: &HashMap<K, V>) -> bool
where
    K: Eq + Hash,
    V: PartialEq,
{
    if a.len() != b.len() {
        return false;
    }
    for (key, value) in a {
        if let Some(other) = b.get(key) {
            if other != value {
                return false;
            }
        }
    }
    true
}

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
    fn get_or_create_works() {
        let mut map: HashMap<&str, i32> = HashMap::new();
        map.insert("a", 1);
        let mut called = 0;
        let value = get_or_create(&mut map, "a", || {
            called += 1;
            99
        });
        assert_eq!(value, 1);
        assert_eq!(called, 0);

        let value = get_or_create(&mut map, "b", || 2);
        assert_eq!(value, 2);
        assert_eq!(map.get("b"), Some(&2));
    }

    #[test]
    fn are_maps_equal_works() {
        let mut a: HashMap<&str, i32> = HashMap::new();
        let mut b: HashMap<&str, i32> = HashMap::new();
        assert!(are_maps_equal(&a, &b));

        a.insert("a", 1);
        b.insert("a", 1);
        assert!(are_maps_equal(&a, &b));

        a.insert("a", 2);
        assert!(!are_maps_equal(&a, &b));

        a.remove("a");
        assert!(!are_maps_equal(&a, &b));
    }

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
