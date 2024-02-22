use std::cmp::min;

use crate::{
    indices::{Direct, Indices, Indirect, Sorted},
    BytesComparable,
};

/// A node in the ART tree, which can be either an inner node or a leaf. Leaf nodes holds data of
/// key-value pairs, and inner nodes holds indices to other nodes.
#[derive(Debug)]
pub enum Node<K, V, const P: usize> {
    Leaf(Box<Leaf<K, V>>),
    Inner(Box<Inner<K, V, P>>),
}

impl<K, V, const P: usize> Node<K, V, P> {
    /// Create a new leaf node.
    pub fn new_leaf(key: K, value: V) -> Self {
        Self::Leaf(Box::new(Leaf { key, value }))
    }

    /// Create a new inner node.
    fn new_inner(partial: PartialKey<P>) -> Self {
        Self::Inner(Box::new(Inner::new(partial)))
    }
}

impl<K, V, const P: usize> Node<K, V, P>
where
    K: BytesComparable,
{
    pub fn search(&self, key: &[u8], depth: usize) -> Option<&Leaf<K, V>> {
        match &self {
            Self::Leaf(leaf) => {
                if leaf.match_key(key) {
                    return Some(leaf);
                }
                None
            }
            Self::Inner(inner) => inner.search_recursive(key, depth),
        }
    }

    pub fn insert(&mut self, key: K, value: V, depth: usize) {
        match self {
            Self::Leaf(leaf) => {
                let (partial, k_new, k_old) = {
                    let new_key_bytes = key.bytes();
                    if leaf.match_key(new_key_bytes.as_ref()) {
                        // Inserting an existing key.
                        leaf.value = value;
                        return;
                    }
                    // Determines the partial key for the new node and the keys for the two children.
                    let old_key_bytes = leaf.key.bytes();
                    let prefix_len = longest_common_prefix(
                        new_key_bytes.as_ref(),
                        old_key_bytes.as_ref(),
                        depth,
                    );
                    let new_depth = depth + prefix_len;
                    (
                        PartialKey::new(&new_key_bytes.as_ref()[depth..], prefix_len),
                        byte_at(new_key_bytes.as_ref(), new_depth),
                        byte_at(old_key_bytes.as_ref(), new_depth),
                    )
                };
                // Replace the current node, then add the old leaf and new leaf as its children.
                let new_leaf = Self::new_leaf(key, value);
                let old_leaf = std::mem::replace(self, Self::new_inner(partial));
                self.add_child(k_new, new_leaf);
                self.add_child(k_old, old_leaf);
            }
            Self::Inner(inner) => {
                if inner.partial.len > 0 {
                    let (prefix_diff, byte_key) = {
                        let key_bytes = key.bytes();
                        let prefix_diff = inner.prefix_mismatch(key_bytes.as_ref(), depth);
                        (
                            prefix_diff,
                            byte_at(key_bytes.as_ref(), depth + prefix_diff),
                        )
                    };
                    if prefix_diff < inner.partial.len {
                        let shift = prefix_diff + 1;
                        let partial = PartialKey::new(&inner.partial.data, prefix_diff);
                        if inner.partial.len <= P {
                            let byte_key = byte_at(&inner.partial.data, prefix_diff);
                            inner.partial.len -= shift;
                            inner.partial.data.copy_within(shift.., 0);
                            let old_node = std::mem::replace(self, Self::new_inner(partial));
                            self.add_child(byte_key, old_node);
                        } else if let Some(leaf) = inner.indices.min_leaf_recursive() {
                            let byte_key = {
                                let leaf_key_bytes = leaf.key.bytes();
                                let offset = depth + shift;
                                let partial_len = min(P, inner.partial.len);
                                inner.partial.len -= shift;
                                inner.partial.data[..partial_len].copy_from_slice(
                                    &leaf_key_bytes.as_ref()[offset..offset + partial_len],
                                );
                                byte_at(leaf_key_bytes.as_ref(), depth + prefix_diff)
                            };
                            let old_node = std::mem::replace(self, Self::new_inner(partial));
                            self.add_child(byte_key, old_node);
                        }
                        let leaf = Self::new_leaf(key, value);
                        self.add_child(byte_key, leaf);
                    } else {
                        inner.insert_recursive(key, value, depth + inner.partial.len);
                    }
                } else {
                    inner.insert_recursive(key, value, depth);
                }
            }
        }
    }

    pub fn delete(&mut self, key: &[u8], depth: usize) -> Option<Self> {
        let Self::Inner(inner) = self else {
            return None;
        };
        let deleted = inner.delete_recursive(key, depth);
        if let Some(node) = inner.shrink() {
            *self = node;
        }
        deleted
    }

    pub fn min_leaf(&self) -> Option<&Leaf<K, V>> {
        match self {
            Self::Leaf(leaf) => Some(leaf),
            Self::Inner(inner) => inner.indices.min_leaf_recursive(),
        }
    }

    pub fn max_leaf(&self) -> Option<&Leaf<K, V>> {
        match self {
            Self::Leaf(leaf) => Some(leaf),
            Self::Inner(inner) => inner.indices.max_leaf_recursive(),
        }
    }

    fn add_child(&mut self, key: u8, child: Self) {
        if let Self::Inner(inner) = self {
            inner.add_child(key, child);
        };
    }
}

pub fn debug_print<K, V, const P: usize>(
    f: &mut std::fmt::Formatter<'_>,
    node: &Node<K, V, P>,
    key: u8,
    level: usize,
) -> std::fmt::Result
where
    K: std::fmt::Debug,
    V: std::fmt::Debug,
{
    for _ in 0..level {
        write!(f, "  ")?;
    }
    match node {
        Node::Leaf(leaf) => {
            writeln!(f, "[{:03}] leaf: {:?} -> {:?}", key, leaf.key, leaf.value)?;
        }
        Node::Inner(inner) => match &inner.indices {
            InnerIndices::Node4(indices) => {
                writeln!(f, "[{:03}] node4 {:?}", key, inner.partial)?;
                for (key, child) in indices {
                    debug_print(f, child, key, level + 1)?;
                }
            }
            InnerIndices::Node16(indices) => {
                writeln!(f, "[{:03}] node16 {:?}", key, inner.partial)?;
                for (key, child) in indices {
                    debug_print(f, child, key, level + 1)?;
                }
            }
            InnerIndices::Node48(indices) => {
                writeln!(f, "[{:03}] node48 {:?}", key, inner.partial)?;
                for (key, child) in indices {
                    debug_print(f, child, key, level + 1)?;
                }
            }
            InnerIndices::Node256(indices) => {
                writeln!(f, "[{:03}] node256 {:?}", key, inner.partial)?;
                for (key, child) in indices {
                    debug_print(f, child, key, level + 1)?;
                }
            }
        },
    }
    Ok(())
}

/// Count the number of matching elements at the beginning of two slices.
fn longest_common_prefix<T>(lhs: &[T], rhs: &[T], depth: usize) -> usize
where
    T: PartialEq,
{
    lhs[depth..]
        .iter()
        .zip(rhs[depth..].iter())
        .take_while(|(x, y)| x == y)
        .count()
}

fn byte_at(bytes: &[u8], pos: usize) -> u8 {
    bytes.get(pos).copied().unwrap_or(0)
}

#[derive(Debug, Clone)]
pub struct Leaf<K, V> {
    pub key: K,
    pub value: V,
}

impl<K, V> Leaf<K, V>
where
    K: BytesComparable,
{
    pub fn match_key(&self, key: &[u8]) -> bool {
        self.key.bytes().as_ref() == key
    }
}

#[derive(Debug)]
pub struct Inner<K, V, const P: usize> {
    partial: PartialKey<P>,
    indices: InnerIndices<K, V, P>,
}

impl<K, V, const P: usize> Inner<K, V, P> {
    fn new(partial: PartialKey<P>) -> Self {
        Self {
            partial,
            indices: InnerIndices::Node4(Sorted::default()),
        }
    }
}

impl<K, V, const P: usize> Inner<K, V, P>
where
    K: BytesComparable,
{
    fn search_recursive(&self, key: &[u8], depth: usize) -> Option<&Leaf<K, V>> {
        if !self.partial.match_key(key, depth) {
            return None;
        }
        let next_depth = depth + self.partial.len;
        let byte_key = byte_at(key, next_depth);
        self.child_ref(byte_key)
            .and_then(|child| child.search(key, next_depth + 1))
    }

    fn insert_recursive(&mut self, key: K, value: V, depth: usize) {
        let byte_key = byte_at(key.bytes().as_ref(), depth);
        if let Some(child) = self.child_mut(byte_key) {
            child.insert(key, value, depth + 1);
        } else {
            let leaf = Node::new_leaf(key, value);
            self.add_child(byte_key, leaf);
        }
    }

    fn delete_recursive(&mut self, key: &[u8], depth: usize) -> Option<Node<K, V, P>> {
        // The key doesn't match the prefix partial.
        if !self.partial.match_key(key, depth) {
            return None;
        }
        // Find the child node corresponding to the key.
        let depth = depth + self.partial.len;
        let child_key = byte_at(key, depth);
        let Some(child) = self.child_mut(child_key) else {
            return None;
        };
        // Do recursion if the child is an inner node.
        let Node::Leaf(leaf) = child else {
            return child.delete(key, depth + 1);
        };
        // The leaf's key doesn't match.
        if !leaf.match_key(key) {
            return None;
        }
        self.del_child(child_key)
    }

    fn add_child(&mut self, key: u8, child: Node<K, V, P>) {
        self.grow();
        match &mut self.indices {
            InnerIndices::Node4(indices) => indices.add_child(key, child),
            InnerIndices::Node16(indices) => indices.add_child(key, child),
            InnerIndices::Node48(indices) => indices.add_child(key, child),
            InnerIndices::Node256(indices) => indices.add_child(key, child),
        }
    }

    fn del_child(&mut self, key: u8) -> Option<Node<K, V, P>> {
        match &mut self.indices {
            InnerIndices::Node4(indices) => indices.del_child(key),
            InnerIndices::Node16(indices) => indices.del_child(key),
            InnerIndices::Node48(indices) => indices.del_child(key),
            InnerIndices::Node256(indices) => indices.del_child(key),
        }
    }

    fn child_ref(&self, key: u8) -> Option<&Node<K, V, P>> {
        match &self.indices {
            InnerIndices::Node4(indices) => indices.child_ref(key),
            InnerIndices::Node16(indices) => indices.child_ref(key),
            InnerIndices::Node48(indices) => indices.child_ref(key),
            InnerIndices::Node256(indices) => indices.child_ref(key),
        }
    }

    fn child_mut(&mut self, key: u8) -> Option<&mut Node<K, V, P>> {
        match &mut self.indices {
            InnerIndices::Node4(indices) => indices.child_mut(key),
            InnerIndices::Node16(indices) => indices.child_mut(key),
            InnerIndices::Node48(indices) => indices.child_mut(key),
            InnerIndices::Node256(indices) => indices.child_mut(key),
        }
    }

    fn grow(&mut self) {
        match &mut self.indices {
            InnerIndices::Node4(indices) => {
                if indices.is_full() {
                    let mut new_indices = Sorted::<Node<K, V, P>, 16>::default();
                    new_indices.consume_sorted(indices);
                    self.indices = InnerIndices::Node16(new_indices);
                }
            }
            InnerIndices::Node16(indices) => {
                if indices.is_full() {
                    let mut new_indices = Indirect::<Node<K, V, P>, 48>::default();
                    new_indices.consume_sorted(indices);
                    self.indices = InnerIndices::Node48(new_indices);
                }
            }
            InnerIndices::Node48(indices) => {
                if indices.is_full() {
                    let mut new_indices = Direct::<Node<K, V, P>>::default();
                    new_indices.consume_indirect(indices);
                    self.indices = InnerIndices::Node256(new_indices);
                }
            }
            InnerIndices::Node256(_) => {}
        }
    }

    fn shrink(&mut self) -> Option<Node<K, V, P>> {
        match &mut self.indices {
            InnerIndices::Node4(indices) => {
                if let Some((sub_child_key, mut sub_child)) = indices.release() {
                    if let Node::Inner(sub_child) = &mut sub_child {
                        self.partial.push(sub_child_key);
                        self.partial.append(&sub_child.partial);
                        std::mem::swap(&mut self.partial, &mut sub_child.partial);
                    }
                    return Some(sub_child);
                }
            }
            InnerIndices::Node16(indices) => {
                if indices.len() < 4 {
                    let mut new_indices = Sorted::<Node<K, V, P>, 4>::default();
                    new_indices.consume_sorted(indices);
                    self.indices = InnerIndices::Node4(new_indices);
                }
            }
            InnerIndices::Node48(indices) => {
                if indices.len() < 16 {
                    let mut new_indices = Sorted::<Node<K, V, P>, 16>::default();
                    new_indices.consume_indirect(indices);
                    self.indices = InnerIndices::Node16(new_indices);
                }
            }
            InnerIndices::Node256(indices) => {
                if indices.len() < 48 {
                    let mut new_indices = Indirect::<Node<K, V, P>, 48>::default();
                    new_indices.consume_direct(indices);
                    self.indices = InnerIndices::Node48(new_indices);
                }
            }
        }
        None
    }

    fn prefix_mismatch(&self, key: &[u8], depth: usize) -> usize {
        let len = min(P, self.partial.len);
        let mut idx = 0;
        for (l, r) in self.partial.data[..len].iter().zip(key[depth..].iter()) {
            if l != r {
                return idx;
            }
            idx += 1;
        }
        // If the prefix is short so we don't have to check a leaf.
        if self.partial.len > P {
            if let Some(leaf) = self.indices.min_leaf_recursive() {
                idx += longest_common_prefix(leaf.key.bytes().as_ref(), key, depth + idx);
            }
        }
        idx
    }
}

#[derive(Debug)]
enum InnerIndices<K, V, const P: usize> {
    Node4(Sorted<Node<K, V, P>, 4>),
    Node16(Sorted<Node<K, V, P>, 16>),
    Node48(Indirect<Node<K, V, P>, 48>),
    Node256(Direct<Node<K, V, P>>),
}

impl<K, V, const P: usize> InnerIndices<K, V, P> {
    fn min_leaf_recursive(&self) -> Option<&Leaf<K, V>> {
        match self {
            Self::Node4(indices) => indices.min(),
            Self::Node16(indices) => indices.min(),
            Self::Node48(indices) => indices.min(),
            Self::Node256(indices) => indices.min(),
        }
        .and_then(|child| match child {
            Node::Leaf(leaf) => Some(leaf.as_ref()),
            Node::Inner(inner) => inner.indices.min_leaf_recursive(),
        })
    }

    fn max_leaf_recursive(&self) -> Option<&Leaf<K, V>> {
        match self {
            Self::Node4(indices) => indices.max(),
            Self::Node16(indices) => indices.max(),
            Self::Node48(indices) => indices.max(),
            Self::Node256(indices) => indices.max(),
        }
        .and_then(|child| match child {
            Node::Leaf(leaf) => Some(leaf.as_ref()),
            Node::Inner(inner) => inner.indices.max_leaf_recursive(),
        })
    }
}

#[derive(Debug, Clone)]
struct PartialKey<const N: usize> {
    len: usize,
    data: [u8; N],
}

impl<const N: usize> PartialKey<N> {
    fn new(key: &[u8], len: usize) -> Self {
        let partial_len = min(N, len);
        let mut data = [0; N];
        data[..partial_len].copy_from_slice(&key[..partial_len]);
        Self { len, data }
    }

    fn push(&mut self, char: u8) {
        if self.len < N {
            self.data[self.len] = char;
        }
        self.len += 1;
    }

    fn append(&mut self, other: &Self) {
        if self.len < N {
            let len = min(other.len, N - self.len);
            self.data[self.len..self.len + len].copy_from_slice(&other.data[..len]);
        }
        self.len += other.len;
    }

    fn match_key(&self, key: &[u8], depth: usize) -> bool {
        let partial_len = min(N, self.len);
        self.data[..partial_len]
            .iter()
            .zip(key[depth..].iter())
            .take_while(|(x, y)| x.eq(y))
            .count()
            .eq(&partial_len)
    }
}