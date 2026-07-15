//! バイト予算つき LRU 会計（Issue #258）。
//!
//! 実体の解放は GPUI など所有側が行う。core はキーごとの推定デコード済み
//! バイト数と参照順だけを管理し、予算超過時に退避すべきキーを返す。

use std::collections::HashMap;
use std::hash::Hash;

#[derive(Debug, Clone, Copy)]
struct Entry {
    bytes: u64,
    touched_at: u64,
}

#[derive(Debug, Clone)]
pub struct ByteLru<K> {
    budget_bytes: u64,
    used_bytes: u64,
    clock: u64,
    entries: HashMap<K, Entry>,
}

impl<K: Clone + Eq + Hash> ByteLru<K> {
    pub fn new(budget_bytes: u64) -> Self {
        Self {
            budget_bytes,
            used_bytes: 0,
            clock: 0,
            entries: HashMap::new(),
        }
    }

    pub fn budget_bytes(&self) -> u64 {
        self.budget_bytes
    }

    pub fn used_bytes(&self) -> u64 {
        self.used_bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// キーを追加または更新し、予算超過で退避したキーを古い順に返す。
    /// 単一エントリが予算を超える場合はそのエントリ自身も退避する。
    pub fn insert(&mut self, key: K, bytes: u64) -> Vec<K> {
        self.clock = self.clock.wrapping_add(1);
        if let Some(previous) = self.entries.insert(
            key,
            Entry {
                bytes,
                touched_at: self.clock,
            },
        ) {
            self.used_bytes = self.used_bytes.saturating_sub(previous.bytes);
        }
        self.used_bytes = self.used_bytes.saturating_add(bytes);
        self.evict_over_budget()
    }

    /// 既存キーを最新参照へ進める。未知キーは追加しない。
    pub fn touch(&mut self, key: &K) -> bool {
        let Some(entry) = self.entries.get_mut(key) else {
            return false;
        };
        self.clock = self.clock.wrapping_add(1);
        entry.touched_at = self.clock;
        true
    }

    pub fn remove(&mut self, key: &K) -> bool {
        let Some(entry) = self.entries.remove(key) else {
            return false;
        };
        self.used_bytes = self.used_bytes.saturating_sub(entry.bytes);
        true
    }

    /// 予算を変更し、超過分として退避すべきキーを返す。
    pub fn set_budget_bytes(&mut self, budget_bytes: u64) -> Vec<K> {
        self.budget_bytes = budget_bytes;
        self.evict_over_budget()
    }

    fn evict_over_budget(&mut self) -> Vec<K> {
        let mut evicted = Vec::new();
        while self.used_bytes > self.budget_bytes {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.touched_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.remove(&oldest);
            evicted.push(oldest);
        }
        evicted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 予算超過時に最終参照が古い順で退避する() {
        let mut lru = ByteLru::new(100);
        assert!(lru.insert("a", 40).is_empty());
        assert!(lru.insert("b", 40).is_empty());
        assert!(lru.touch(&"a"));
        assert_eq!(lru.insert("c", 40), vec!["b"]);
        assert_eq!(lru.used_bytes(), 80);
        assert_eq!(lru.len(), 2);
    }

    #[test]
    fn 予算縮小と単一超過も上限を守る() {
        let mut lru = ByteLru::new(100);
        lru.insert(1, 40);
        lru.insert(2, 40);
        assert_eq!(lru.set_budget_bytes(30), vec![1, 2]);
        assert!(lru.is_empty());
        assert_eq!(lru.used_bytes(), 0);
        assert_eq!(lru.insert(3, 31), vec![3]);
        assert_eq!(lru.used_bytes(), 0);
    }

    #[test]
    fn 同じキーの更新は使用量を二重計上しない() {
        let mut lru = ByteLru::new(100);
        lru.insert("a", 40);
        assert!(lru.insert("a", 60).is_empty());
        assert_eq!(lru.used_bytes(), 60);
        assert_eq!(lru.len(), 1);
        assert!(lru.remove(&"a"));
        assert_eq!(lru.used_bytes(), 0);
    }
}
