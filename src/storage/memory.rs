//! An in-memory storage for [`Todo`] objects

use super::{Result, Todo, TodoStore};
use async_trait::async_trait;
use futures::lock::Mutex;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// An in-memory storage, keeping all entries in a [`BTreeMap`]
///
/// Since it is shared between threads and accessed concurrently by different
/// requests, it needs to be thread-safe. The compiler enforces this, as we can only
/// get an immutable reference to the shared data so we need to use interior mutability
/// to actually change the stored data.
pub struct MemoryStorage {
    /// the id of the next created entry
    counter: AtomicUsize,

    /// the actual map containing the entries
    ///
    /// Since the [`BTreeMap`] type is not [`Sync`] (i.e. thread-safe), we need to serialize
    /// access to it. Note that the mutex type used here is [`futures::lock::Mutex`], not
    /// [`std::sync::Mutex`]. The difference from the user perspective is that the futures-aware
    /// mutex needs to be `await`ed instead of blocking the calling thread.
    store: Mutex<BTreeMap<usize, Todo>>,
}

// if your type has a constructor that doesn't take any parameters, it's good practice
// to implement the [`Default`] trait instead
impl Default for MemoryStorage {
    fn default() -> Self {
        Self {
            counter: AtomicUsize::new(1),
            store: Default::default(),
        }
    }
}

#[async_trait]
impl TodoStore for Arc<MemoryStorage> {
    async fn create(&self, todo: Todo) -> Result<usize> {
        let mut store = self.store.lock().await;
        let id = self.counter.fetch_add(1, Ordering::Acquire);

        store.insert(id, todo);
        Ok(id)
    }

    async fn list(&self) -> Result<BTreeMap<usize, Todo>> {
        let store = self.store.lock().await;

        Ok(store.clone())
    }

    async fn mark_done(&self, id: usize) -> Result<bool> {
        let mut store = self.store.lock().await;

        if let Some(todo) = store.get_mut(&id) {
            todo.mark_done();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn delete(&self, id: usize) -> Result<bool> {
        let mut store = self.store.lock().await;

        Ok(store.remove(&id).is_some())
    }
}
