//! Canonical Merkle digest tree for drawing content keyed by typed `u64` IDs.
//!
//! The tree is a compressed binary Patricia trie over a 128-bit key: the
//! domain occupies the high 64 bits and the entity/layer/import ID the low
//! 64 bits. Branches are split at the highest differing bit, so the shape and
//! root digest depend only on the current key/value set, not update history.

use sha2::{Digest, Sha256};
use std::sync::{Arc, OnceLock};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub(crate) enum DigestDomain {
    Entity = 0,
    Layer = 1,
    ImportAsset = 2,
}

impl DigestDomain {
    fn key(self, id: u64) -> u128 {
        ((self as u128) << 64) | u128::from(id)
    }
}

/// The persistent content digest map. Its nodes are either leaves or binary
/// branches; a map with `n` leaves therefore has at most `2*n - 1` allocated
/// nodes. The empty root is a separately tagged digest, not an allocated node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContentDigestTree {
    root: Option<Arc<Node>>,
    len: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Node {
    Leaf {
        key: u128,
        value_hash: [u8; 32],
        hash: [u8; 32],
    },
    Branch {
        split_bit: u8,
        left: Arc<Node>,
        right: Arc<Node>,
        hash: [u8; 32],
    },
}

impl Node {
    fn hash(&self) -> &[u8; 32] {
        match self {
            Self::Leaf { hash, .. } | Self::Branch { hash, .. } => hash,
        }
    }
}

impl Default for ContentDigestTree {
    fn default() -> Self {
        Self::new()
    }
}

impl ContentDigestTree {
    pub(crate) fn new() -> Self {
        Self { root: None, len: 0 }
    }

    /// Insert or replace a content leaf and return the number of Merkle node
    /// hashes recomputed. An unchanged leaf returns zero.
    pub(crate) fn upsert(
        &mut self,
        domain: DigestDomain,
        id: u64,
        leaf_hash: [u8; 32],
    ) -> u64 {
        let key = domain.key(id);
        let candidate = self
            .root
            .as_deref()
            .map(|root| find_leaf(root, key));

        if let Some((candidate_key, existing_value)) = candidate {
            if candidate_key == key && existing_value == leaf_hash {
                return 0;
            }
        }

        let mut hashes = 0;
        let Some(root) = self.root.take() else {
            self.root = Some(make_leaf(key, leaf_hash, &mut hashes));
            self.len = 1;
            return hashes;
        };

        match candidate {
            Some((candidate_key, _)) if candidate_key == key => {
                self.root = Some(replace_leaf(root, key, leaf_hash, &mut hashes));
            }
            Some((candidate_key, _)) => {
                let different_bits = key ^ candidate_key;
                debug_assert_ne!(different_bits, 0);
                let split_bit = (u128::BITS - 1 - different_bits.leading_zeros()) as u8;
                let new_leaf = make_leaf(key, leaf_hash, &mut hashes);
                let mut new_leaf = Some(new_leaf);
                self.root = Some(insert_leaf(
                    root,
                    key,
                    candidate_key,
                    split_bit,
                    &mut new_leaf,
                    &mut hashes,
                ));
                debug_assert!(new_leaf.is_none());
                self.len += 1;
            }
            None => unreachable!("a non-empty tree always has a candidate leaf"),
        }

        hashes
    }

    /// Remove a leaf and return the number of surviving branch hashes
    /// recomputed. Removing an absent key returns zero.
    pub(crate) fn remove(&mut self, domain: DigestDomain, id: u64) -> u64 {
        let key = domain.key(id);
        let exists = self
            .root
            .as_deref()
            .is_some_and(|root| find_leaf(root, key).0 == key);
        if !exists {
            return 0;
        }

        let mut hashes = 0;
        self.root = remove_leaf(
            self.root.take().expect("existing leaf has a root"),
            key,
            &mut hashes,
        );
        self.len -= 1;
        hashes
    }

    pub(crate) fn root_hash(&self) -> [u8; 32] {
        self.root
            .as_deref()
            .map(|root| *root.hash())
            .unwrap_or_else(empty_root_hash)
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }
}

fn find_leaf(node: &Node, key: u128) -> (u128, [u8; 32]) {
    match node {
        Node::Leaf {
            key, value_hash, ..
        } => (*key, *value_hash),
        Node::Branch {
            split_bit,
            left,
            right,
            ..
        } => {
            if bit_is_set(key, *split_bit) {
                find_leaf(right, key)
            } else {
                find_leaf(left, key)
            }
        }
    }
}

fn bit_is_set(key: u128, bit: u8) -> bool {
    ((key >> bit) & 1) != 0
}

fn make_leaf(key: u128, value_hash: [u8; 32], hashes: &mut u64) -> Arc<Node> {
    let mut bytes = [0u8; 48];
    bytes[..16].copy_from_slice(&key.to_be_bytes());
    bytes[16..].copy_from_slice(&value_hash);
    let hash = hash_bytes("content-digest.leaf.v1", &bytes);
    *hashes += 1;
    Arc::new(Node::Leaf {
        key,
        value_hash,
        hash,
    })
}

fn make_branch(
    split_bit: u8,
    left: Arc<Node>,
    right: Arc<Node>,
    hashes: &mut u64,
) -> Arc<Node> {
    let mut bytes = [0u8; 65];
    bytes[0] = split_bit;
    bytes[1..33].copy_from_slice(left.hash());
    bytes[33..].copy_from_slice(right.hash());
    let hash = hash_bytes("content-digest.branch.v1", &bytes);
    *hashes += 1;
    Arc::new(Node::Branch {
        split_bit,
        left,
        right,
        hash,
    })
}

fn replace_leaf(
    node: Arc<Node>,
    key: u128,
    value_hash: [u8; 32],
    hashes: &mut u64,
) -> Arc<Node> {
    match node.as_ref() {
        Node::Leaf { key: found, .. } => {
            debug_assert_eq!(*found, key);
            make_leaf(key, value_hash, hashes)
        }
        Node::Branch {
            split_bit,
            left,
            right,
            ..
        } => {
            if bit_is_set(key, *split_bit) {
                make_branch(
                    *split_bit,
                    Arc::clone(left),
                    replace_leaf(Arc::clone(right), key, value_hash, hashes),
                    hashes,
                )
            } else {
                make_branch(
                    *split_bit,
                    replace_leaf(Arc::clone(left), key, value_hash, hashes),
                    Arc::clone(right),
                    hashes,
                )
            }
        }
    }
}

fn insert_leaf(
    node: Arc<Node>,
    key: u128,
    candidate_key: u128,
    split_bit: u8,
    new_leaf: &mut Option<Arc<Node>>,
    hashes: &mut u64,
) -> Arc<Node> {
    match node.as_ref() {
        Node::Leaf {
            key: existing_key,
            ..
        } => {
            debug_assert_ne!(*existing_key, key);
            debug_assert_eq!(highest_differing_bit(*existing_key, key), split_bit);
            let leaf = new_leaf.take().expect("new leaf is inserted once");
            if bit_is_set(key, split_bit) {
                make_branch(split_bit, Arc::clone(&node), leaf, hashes)
            } else {
                make_branch(split_bit, leaf, Arc::clone(&node), hashes)
            }
        }
        Node::Branch {
            split_bit: current_split,
            left,
            right,
            ..
        } => {
            if split_bit > *current_split {
                let leaf = new_leaf.take().expect("new leaf is inserted once");
                let existing_subtree = Arc::clone(&node);
                debug_assert_ne!(
                    bit_is_set(key, split_bit),
                    bit_is_set(candidate_key, split_bit)
                );
                if bit_is_set(key, split_bit) {
                    make_branch(split_bit, existing_subtree, leaf, hashes)
                } else {
                    make_branch(split_bit, leaf, existing_subtree, hashes)
                }
            } else {
                debug_assert!(split_bit < *current_split);
                if bit_is_set(key, *current_split) {
                    make_branch(
                        *current_split,
                        Arc::clone(left),
                        insert_leaf(
                            Arc::clone(right),
                            key,
                            candidate_key,
                            split_bit,
                            new_leaf,
                            hashes,
                        ),
                        hashes,
                    )
                } else {
                    make_branch(
                        *current_split,
                        insert_leaf(
                            Arc::clone(left),
                            key,
                            candidate_key,
                            split_bit,
                            new_leaf,
                            hashes,
                        ),
                        Arc::clone(right),
                        hashes,
                    )
                }
            }
        }
    }
}

fn highest_differing_bit(a: u128, b: u128) -> u8 {
    debug_assert_ne!(a, b);
    (u128::BITS - 1 - (a ^ b).leading_zeros()) as u8
}

fn remove_leaf(node: Arc<Node>, key: u128, hashes: &mut u64) -> Option<Arc<Node>> {
    match node.as_ref() {
        Node::Leaf { key: found, .. } => {
            debug_assert_eq!(*found, key);
            None
        }
        Node::Branch {
            split_bit,
            left,
            right,
            ..
        } => {
            if bit_is_set(key, *split_bit) {
                match remove_leaf(Arc::clone(right), key, hashes) {
                    None => Some(Arc::clone(left)),
                    Some(right) => Some(make_branch(
                        *split_bit,
                        Arc::clone(left),
                        right,
                        hashes,
                    )),
                }
            } else {
                match remove_leaf(Arc::clone(left), key, hashes) {
                    None => Some(Arc::clone(right)),
                    Some(left) => Some(make_branch(
                        *split_bit,
                        left,
                        Arc::clone(right),
                        hashes,
                    )),
                }
            }
        }
    }
}

/// Hash bytes with a stable, length-delimited domain separator.
pub(crate) fn hash_bytes(domain_tag: &str, bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"drafting-lab\0content-digest\0v1\0");
    hasher.update((domain_tag.len() as u64).to_be_bytes());
    hasher.update(domain_tag.as_bytes());
    hasher.update(bytes);
    hasher.finalize().into()
}

/// Render a digest as lowercase hexadecimal without changing its byte order.
pub(crate) fn hex_digest(hash: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for &byte in hash {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn empty_root_hash() -> [u8; 32] {
    static EMPTY_ROOT: OnceLock<[u8; 32]> = OnceLock::new();
    *EMPTY_ROOT.get_or_init(|| hash_bytes("content-digest.empty-root.v1", &[]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOMAINS: [DigestDomain; 3] = [
        DigestDomain::Entity,
        DigestDomain::Layer,
        DigestDomain::ImportAsset,
    ];

    fn value(id: u64, generation: u8) -> [u8; 32] {
        let mut bytes = id.to_be_bytes().to_vec();
        bytes.push(generation);
        hash_bytes("test-content-leaf", &bytes)
    }

    #[test]
    fn root_is_independent_of_insertion_order() {
        let entries = [
            (DigestDomain::Entity, 7, value(7, 1)),
            (DigestDomain::Layer, 7, value(7, 2)),
            (DigestDomain::ImportAsset, 0, value(0, 3)),
            (DigestDomain::Entity, u64::MAX, value(u64::MAX, 4)),
            (DigestDomain::Layer, 1 << 63, value(1 << 63, 5)),
        ];
        let mut forward = ContentDigestTree::new();
        for (domain, id, digest) in entries {
            forward.upsert(domain, id, digest);
        }
        let mut reverse = ContentDigestTree::new();
        for (domain, id, digest) in entries.into_iter().rev() {
            reverse.upsert(domain, id, digest);
        }

        assert_eq!(forward.len(), entries.len());
        assert_eq!(forward.root_hash(), reverse.root_hash());
    }

    #[test]
    fn update_and_remove_match_a_cold_rebuild() {
        let mut warm = ContentDigestTree::new();
        for id in 0..32 {
            warm.upsert(DigestDomain::Entity, id, value(id, 1));
        }
        warm.upsert(DigestDomain::Entity, 9, value(9, 2));
        warm.remove(DigestDomain::Entity, 3);
        warm.remove(DigestDomain::Layer, 3);

        let mut cold = ContentDigestTree::new();
        for id in (0..32).filter(|id| *id != 3 && *id != 9) {
            cold.upsert(DigestDomain::Entity, id, value(id, 1));
        }
        cold.upsert(DigestDomain::Entity, 9, value(9, 2));

        assert_eq!(warm.len(), cold.len());
        assert_eq!(warm.root_hash(), cold.root_hash());
        assert!(warm.remove(DigestDomain::Entity, 9) > 0);
        assert_eq!(warm.remove(DigestDomain::Entity, 9), 0);
    }

    #[test]
    fn identical_numeric_ids_in_different_domains_are_distinct_keys() {
        let leaf = value(42, 1);
        let mut entity_only = ContentDigestTree::new();
        entity_only.upsert(DigestDomain::Entity, 42, leaf);
        let mut layer_only = ContentDigestTree::new();
        layer_only.upsert(DigestDomain::Layer, 42, leaf);
        let mut mixed = ContentDigestTree::new();
        mixed.upsert(DigestDomain::Entity, 42, leaf);
        mixed.upsert(DigestDomain::Layer, 42, leaf);

        assert_eq!(mixed.len(), 2);
        assert_ne!(entity_only.root_hash(), layer_only.root_hash());
        assert_ne!(mixed.root_hash(), entity_only.root_hash());
        assert_ne!(mixed.root_hash(), layer_only.root_hash());
    }

    #[test]
    fn one_update_rehashes_at_most_67_nodes_with_twenty_thousand_leaves() {
        let mut tree = ContentDigestTree::new();
        for domain in DOMAINS {
            for id in 0..6_667 {
                tree.upsert(domain, id, value(id, domain as u8));
            }
            for bit in 13..64 {
                let id = 1u64 << bit;
                tree.upsert(domain, id, value(id, domain as u8));
            }
        }
        assert!(tree.len() >= 20_000);

        // The zero key follows both varying domain bits and every ID bit.
        // Its changed leaf therefore exercises the full 66-branch key depth.
        let work = tree.upsert(DigestDomain::Entity, 0, value(0, 99));
        assert_eq!(work, 67);
    }

    #[test]
    fn digest_helpers_are_tagged_and_lowercase() {
        let raw = [0xab; 32];
        assert_eq!(hex_digest(&raw), "ab".repeat(32));
        assert_ne!(
            hash_bytes("entity", b"payload"),
            hash_bytes("layer", b"payload")
        );
    }
}
