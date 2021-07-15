use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;
use tokio::task::JoinError;

pub mod memory;
pub mod redis;

/// The errors that can happen when accessing the storage
#[derive(Error, Debug)]
pub enum TodoError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Redis error: {0}")]
    Redis(#[from] ::redis::RedisError),

    #[error("JSON (de)serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Failed to join spawned task: {0}")]
    TaskJoin(#[from] JoinError),
}

/// all our methods return TodoError as the error type
/// so create an alias to save some typing
pub type Result<T> = std::result::Result<T, TodoError>;

/// Entry status
///
/// Each todo entry is either pending (not yet completed) or done
/// We derive the following trait implementations:
/// - [`Clone`]: needed for Copy
/// - [`Copy`]: since this is a small and simple type, don't require
///   explicit `.clone()` calls to copy them
/// - [`Eq`], [`PartialEq`]: needed for comparing for equality
/// - [`Deserialize`], [`Serialize`]: for transparent conversion from/to JSON
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum TodoStatus {
    Pending,
    Done,
}

/// Todo entry
///
/// A single entry in the database. It contains a title and a status (pending/completed)
/// We derive mostly the same implementations as for [`TodoStatus`] but note that we cannot
/// derive a [`Copy`] implementation since [`String`] does not implement it
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Todo {
    title: String,
    status: TodoStatus,
}

impl Todo {
    /// Create a new todo entry
    ///
    /// We take the title by ownership (potentially forcing the caller to make a copy)
    /// since otherwise we would have to clone the string unconditionally
    pub fn new(title: String) -> Self {
        Self {
            title,
            status: TodoStatus::Pending,
        }
    }

    /// Mark a todo entry as done
    pub fn mark_done(&mut self) {
        self.status = TodoStatus::Done;
    }

    #[cfg(test)]
    /// Get todo status
    ///
    /// This method is only used by the tests and isn't even compiled
    /// outside the test runs
    pub fn get_status(&self) -> TodoStatus {
        self.status
    }
}

// [`async_trait`] is a workaround for the lack of asynchronous methods in traits.
// It's suboptimal (forces a heap allocation) but otherwise very useful

/// A trait for the data store
///
/// Each data store must implement these methods itself (there are no default
/// implementations)
///
/// *Note*: the types in the documentation are different from the source code.
/// They have been rewritten by [`mod@async_trait`].
#[async_trait]
pub trait TodoStore {
    /// Store a todo entry
    ///
    /// The successful return value is a unique numeric id of the new entry
    async fn create(&self, todo: Todo) -> Result<usize>;

    /// List all entries
    ///
    /// The successful result is a map of ids to [`Todo`] objects
    async fn list(&self) -> Result<BTreeMap<usize, Todo>>;

    /// Mark a specific todo entry as done
    ///
    /// Returns Ok(true) if the entry was changed, Ok(false) if it didn't exist
    async fn mark_done(&self, id: usize) -> Result<bool>;

    /// Delete a todo entry
    ///
    /// Returns Ok(true) if the entry was deleted, Ok(false) if it didn't exist
    async fn delete(&self, id: usize) -> Result<bool>;
}
