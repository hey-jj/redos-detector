//! Insertion-ordered trie that keeps only the deepest unique values.
//!
//! A value is decoded into a sequence of keys. Adding a sequence stores the
//! value at its final node. When a longer sequence passes through a node that
//! held a value, that value is shadowed and dropped. `items` returns the live
//! values in insertion order.

use std::collections::HashMap;
use std::hash::Hash;

struct TreeNode<K> {
    children: HashMap<K, usize>,
    result_slot: Option<usize>,
}

/// A function that decodes a value into its key sequence.
type Decode<T, K> = Box<dyn Fn(&T) -> Vec<K>>;

/// A trie keyed by the decoded sequence of each value.
pub(crate) struct Tree<T, K: Eq + Hash + Clone> {
    decode: Decode<T, K>,
    nodes: Vec<TreeNode<K>>,
    results: Vec<Option<T>>,
}

impl<T, K: Eq + Hash + Clone> Tree<T, K> {
    /// Builds a tree using `decode` to turn a value into its key sequence.
    pub(crate) fn new(decode: impl Fn(&T) -> Vec<K> + 'static) -> Self {
        Tree {
            decode: Box::new(decode),
            nodes: vec![TreeNode {
                children: HashMap::new(),
                result_slot: None,
            }],
            results: Vec::new(),
        }
    }

    /// Inserts a value. Adding an empty sequence is a no-op.
    pub(crate) fn add(&mut self, input: T) {
        let values = (self.decode)(&input);
        if values.is_empty() {
            return;
        }

        let mut current = 0usize;
        let num = values.len();
        for (i, value) in values.into_iter().enumerate() {
            let last = i == num - 1;
            let existing = self.nodes[current].children.get(&value).copied();
            match existing {
                Some(child) => {
                    current = child;
                    if !last {
                        if let Some(slot) = self.nodes[current].result_slot.take() {
                            self.results[slot] = None;
                        }
                    }
                }
                None => {
                    let new_id = self.nodes.len();
                    self.nodes.push(TreeNode {
                        children: HashMap::new(),
                        result_slot: None,
                    });
                    self.nodes[current].children.insert(value, new_id);
                    current = new_id;
                    if last {
                        let slot = self.results.len();
                        self.results.push(Some(input));
                        self.nodes[current].result_slot = Some(slot);
                        return;
                    }
                }
            }
        }
    }

    /// Returns the live values in insertion order.
    pub(crate) fn items(&self) -> Vec<&T> {
        self.results.iter().filter_map(|r| r.as_ref()).collect()
    }

    /// Iterates the live values in insertion order without allocating.
    pub(crate) fn iter_items(&self) -> impl Iterator<Item = &T> {
        self.results.iter().filter_map(|r| r.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn works_with_tokens() {
        let mut tree: Tree<Vec<u32>, u32> = Tree::new(|x: &Vec<u32>| x.clone());
        let a = 0u32;
        let b = 1u32;
        let c = 2u32;
        assert!(tree.items().is_empty());

        tree.add(vec![]);
        assert!(tree.items().is_empty());

        tree.add(vec![a]);
        assert_eq!(tree.items(), vec![&vec![a]]);

        tree.add(vec![a]);
        assert_eq!(tree.items(), vec![&vec![a]]);

        tree.add(vec![b]);
        assert_eq!(tree.items(), vec![&vec![a], &vec![b]]);

        tree.add(vec![a, b]);
        assert_eq!(tree.items(), vec![&vec![b], &vec![a, b]]);

        tree.add(vec![a, c]);
        assert_eq!(tree.items(), vec![&vec![b], &vec![a, b], &vec![a, c]]);
    }

    #[test]
    fn works_with_decoder() {
        #[derive(PartialEq, Debug, Clone)]
        struct V {
            v: Vec<u32>,
        }
        let mut tree: Tree<V, u32> = Tree::new(|x: &V| x.v.clone());
        let a = 0u32;
        let b = 1u32;
        assert!(tree.items().is_empty());

        let v1 = V { v: vec![a] };
        tree.add(v1.clone());
        assert_eq!(tree.items(), vec![&v1]);

        tree.add(v1.clone());
        assert_eq!(tree.items(), vec![&v1]);

        let v2 = V { v: vec![b] };
        tree.add(v2.clone());
        assert_eq!(tree.items(), vec![&v1, &v2]);

        let v3 = V { v: vec![a, b] };
        tree.add(v3.clone());
        assert_eq!(tree.items(), vec![&v2, &v3]);
    }
}
