use crate::{
    walker::TrieWalker, BranchNodeCompact, HashBuilder, Nibbles, StorageTrieEntry,
    StoredBranchNode, StoredNibbles, StoredNibblesSubKey,
};
use derive_more::Deref;
use reth_db::tables;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::B256;
use std::collections::{hash_map::IntoIter, HashMap, HashSet};

/// The key of a trie node.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TrieKey {
    /// A node in the account trie.
    AccountNode(Nibbles),
    /// A node in the storage trie.
    StorageNode(B256, Nibbles),
    /// Storage trie of an account.
    StorageTrie(B256),
}

impl TrieKey {
    /// Returns reference to account node key if the key is for [`Self::AccountNode`].
    pub const fn as_account_node_key(&self) -> Option<&Nibbles> {
        if let Self::AccountNode(nibbles) = &self {
            Some(nibbles)
        } else {
            None
        }
    }

    /// Returns reference to storage node key if the key is for [`Self::StorageNode`].
    pub const fn as_storage_node_key(&self) -> Option<(&B256, &Nibbles)> {
        if let Self::StorageNode(key, subkey) = &self {
            Some((key, subkey))
        } else {
            None
        }
    }

    /// Returns reference to storage trie key if the key is for [`Self::StorageTrie`].
    pub const fn as_storage_trie_key(&self) -> Option<&B256> {
        if let Self::StorageTrie(key) = &self {
            Some(key)
        } else {
            None
        }
    }
}

/// The operation to perform on the trie.
#[derive(PartialEq, Eq, Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TrieOp {
    /// Delete the node entry.
    Delete,
    /// Update the node entry with the provided value.
    Update(BranchNodeCompact),
}

impl TrieOp {
    /// Returns `true` if the operation is an update.
    pub const fn is_update(&self) -> bool {
        matches!(self, Self::Update(..))
    }

    /// Returns reference to updated branch node if operation is [`Self::Update`].
    pub const fn as_update(&self) -> Option<&BranchNodeCompact> {
        if let Self::Update(node) = &self {
            Some(node)
        } else {
            None
        }
    }

    /// Returns owned updated branch node if operation is [`Self::Update`].
    pub fn into_update(self) -> Option<BranchNodeCompact> {
        if let Self::Update(node) = self {
            Some(node)
        } else {
            None
        }
    }
}

/// The aggregation of trie updates.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deref)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TrieUpdates {
    trie_operations: HashMap<TrieKey, TrieOp>,
}

impl<const N: usize> From<[(TrieKey, TrieOp); N]> for TrieUpdates {
    fn from(value: [(TrieKey, TrieOp); N]) -> Self {
        Self { trie_operations: HashMap::from(value) }
    }
}

impl IntoIterator for TrieUpdates {
    type Item = (TrieKey, TrieOp);
    type IntoIter = IntoIter<TrieKey, TrieOp>;

    fn into_iter(self) -> Self::IntoIter {
        self.trie_operations.into_iter()
    }
}

impl TrieUpdates {
    /// Extend the updates with trie updates.
    pub fn extend(&mut self, updates: impl IntoIterator<Item = (TrieKey, TrieOp)>) {
        self.trie_operations.extend(updates);
    }

    /// Extend the updates with account trie updates.
    pub fn extend_with_account_updates(&mut self, updates: HashMap<Nibbles, BranchNodeCompact>) {
        self.extend(
            updates
                .into_iter()
                .map(|(nibbles, node)| (TrieKey::AccountNode(nibbles), TrieOp::Update(node))),
        );
    }

    /// Finalize state trie updates.
    pub fn finalize_state_updates<C>(
        &mut self,
        walker: TrieWalker<C>,
        hash_builder: HashBuilder,
        destroyed_accounts: HashSet<B256>,
    ) {
        // Add updates from trie walker.
        let (_, deleted_keys) = walker.split();
        self.extend(deleted_keys.into_iter().map(|key| (key, TrieOp::Delete)));

        // Add account node updates from hash builder.
        let (_, hash_builder_updates) = hash_builder.split();
        self.extend_with_account_updates(hash_builder_updates);

        // Add deleted storage tries for destroyed accounts.
        self.extend(
            destroyed_accounts.into_iter().map(|key| (TrieKey::StorageTrie(key), TrieOp::Delete)),
        );
    }

    /// Finalize storage trie updates for a given address.
    pub fn finalize_storage_updates<C>(
        &mut self,
        hashed_address: B256,
        walker: TrieWalker<C>,
        hash_builder: HashBuilder,
    ) {
        // Add updates from trie walker.
        let (_, deleted_keys) = walker.split();
        self.extend(deleted_keys.into_iter().map(|key| (key, TrieOp::Delete)));

        // Add storage node updates from hash builder.
        let (_, hash_builder_updates) = hash_builder.split();
        self.extend(hash_builder_updates.into_iter().map(|(nibbles, node)| {
            (TrieKey::StorageNode(hashed_address, nibbles), TrieOp::Update(node))
        }));
    }

    /// Flush updates all aggregated updates to the database.
    pub fn flush(self, tx: &(impl DbTx + DbTxMut)) -> Result<(), reth_db::DatabaseError> {
        if self.trie_operations.is_empty() {
            return Ok(())
        }

        let mut account_trie_cursor = tx.cursor_write::<tables::AccountsTrie>()?;
        let mut storage_trie_cursor = tx.cursor_dup_write::<tables::StoragesTrie>()?;

        let mut trie_operations = Vec::from_iter(self.trie_operations);
        trie_operations.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        for (key, operation) in trie_operations {
            match key {
                TrieKey::AccountNode(nibbles) => {
                    let nibbles = StoredNibbles(nibbles);
                    match operation {
                        TrieOp::Delete => {
                            if account_trie_cursor.seek_exact(nibbles)?.is_some() {
                                account_trie_cursor.delete_current()?;
                            }
                        }
                        TrieOp::Update(node) => {
                            if !nibbles.0.is_empty() {
                                account_trie_cursor.upsert(nibbles, StoredBranchNode(node))?;
                            }
                        }
                    }
                }
                TrieKey::StorageTrie(hashed_address) => match operation {
                    TrieOp::Delete => {
                        if storage_trie_cursor.seek_exact(hashed_address)?.is_some() {
                            storage_trie_cursor.delete_current_duplicates()?;
                        }
                    }
                    TrieOp::Update(..) => unreachable!("Cannot update full storage trie."),
                },
                TrieKey::StorageNode(hashed_address, nibbles) => {
                    if !nibbles.is_empty() {
                        let nibbles = StoredNibblesSubKey(nibbles);
                        // Delete the old entry if it exists.
                        if storage_trie_cursor
                            .seek_by_key_subkey(hashed_address, nibbles.clone())?
                            .filter(|e| e.nibbles == nibbles)
                            .is_some()
                        {
                            storage_trie_cursor.delete_current()?;
                        }

                        // The operation is an update, insert new entry.
                        if let TrieOp::Update(node) = operation {
                            storage_trie_cursor
                                .upsert(hashed_address, StorageTrieEntry { nibbles, node })?;
                        }
                    }
                }
            };
        }

        Ok(())
    }

    /// creates [`TrieUpdatesSorted`] by sorting the `trie_operations`.
    pub fn sorted(&self) -> TrieUpdatesSorted {
        let mut trie_operations = Vec::from_iter(self.trie_operations.clone());
        trie_operations.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        TrieUpdatesSorted { trie_operations }
    }

    /// converts trie updates into [`TrieUpdatesSorted`].
    pub fn into_sorted(self) -> TrieUpdatesSorted {
        let mut trie_operations = Vec::from_iter(self.trie_operations);
        trie_operations.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        TrieUpdatesSorted { trie_operations }
    }
}

/// The aggregation of trie updates.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deref)]
pub struct TrieUpdatesSorted {
    /// Sorted collection of trie operations.
    pub(crate) trie_operations: Vec<(TrieKey, TrieOp)>,
}

impl TrieUpdatesSorted {
    /// Find the account node with the given nibbles.
    pub fn find_account_node(&self, key: &Nibbles) -> Option<(TrieKey, TrieOp)> {
        self.trie_operations
            .iter()
            .find(|(k, _)| matches!(k, TrieKey::AccountNode(nibbles) if nibbles == key))
            .cloned()
    }

    /// Find the storage node with the given hashed address and key.
    pub fn find_storage_node(
        &self,
        hashed_address: &B256,
        key: &Nibbles,
    ) -> Option<(TrieKey, TrieOp)> {
        self.trie_operations.iter().find(|(k, _)| {
            matches!(k, TrieKey::StorageNode(address, nibbles) if address == hashed_address && nibbles == key)
        }).cloned()
    }
}
