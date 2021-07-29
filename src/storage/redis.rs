//! Redis-backed Todo storage engine

use super::{Result, Todo, TodoStore};
use async_trait::async_trait;
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, RedisResult};
use std::collections::BTreeMap;

/// A zero-sized struct used to indicate an invalid todo id
///
/// We expect all ids stored in Redis to be of the form `todo:<id>`
/// This type is returned as the error variant when we encounter
/// a different form of keys
#[derive(Debug, PartialEq)]
struct TodoIdParseError;

/// Parse a string containing a todo id
///
/// This function returns either `Ok(<id>)` when `todo_id_str` is a valid id
/// or `Err(TodoIdParseError)` when it's not
fn parse_todo_id(todo_id_str: &str) -> std::result::Result<usize, TodoIdParseError> {
    // find the next `:` character
    // [`str::split`] returns an iterator over substrings of the original string,
    // each delimited by the specified separator
    let mut split_todo_id = todo_id_str.split(':');

    // take the next entry from the iterator (i.e. the first `:`-delimited segment)
    // it can be either `Some(string)` or `None` (if we ran out of segments)
    if split_todo_id.next() != Some("todo") {
        // the first segment was not "todo", return a parse error
        return Err(TodoIdParseError);
    }

    // the next segment is the numeric todo id represented as a string
    // store it in `id_str` or return an error if there was none (the id was `todo:`)
    let id_str = split_todo_id.next().ok_or(TodoIdParseError)?;

    // parse the number in `id_str` to an `usize` or return an error
    let id = str::parse::<usize>(id_str).map_err(|_| TodoIdParseError)?;

    if split_todo_id.next().is_some() {
        // there were further segments after the numeric id (e.g. `todo:1:tail`)
        // treat it as invalid
        Err(TodoIdParseError)
    } else {
        Ok(id)
    }

    // note that throughout this function the original string has never been copied,
    // whether in its entirety or in fragments
}

#[derive(Clone)]
pub struct RedisStorage {
    connection: ConnectionManager,
}

impl RedisStorage {
    /// Create a new Redis storage
    ///
    /// Since it involves I/O, this method is both asynchronous and fallible
    pub async fn new(uri: &str) -> Result<Self> {
        let client = redis::Client::open(uri)?;
        let connection = client.get_connection_manager().await?;

        Ok(Self { connection })
    }
}

#[async_trait]
impl TodoStore for RedisStorage {
    /// Write a new [`Todo`] to Redis
    ///
    /// We need to pass a mutable [`ConnectionManager`] to call methods on it. Since we only have
    /// an immutable reference to our storage (because it's shared across threads), we would
    /// normally need to wrap the field in a mutex. However, [`ConnectionManager`] handles
    /// multithreaded access internally and can be cheaply cloned (which amounts to basically
    /// a reference count increase)
    async fn create(&self, todo: Todo) -> Result<usize> {
        let mut conn = self.connection.clone();

        // The `todo_counter` key holds the next id to be allocated
        let todo_id = conn.incr("todo_counter", 1usize).await?;

        // We store individual entries under the key `todo:<id>`...
        let todo_id_str = format!("todo:{}", todo_id);

        // ...with the value being a JSON representation of the [`Todo`] object
        let todo_str = serde_json::to_string(&todo)?;

        // add the new id to the set of known entries...
        conn.sadd("todos", &todo_id_str).await?;
        // ...and store the entry itself
        conn.set(todo_id_str, todo_str).await?;

        Ok(todo_id)
    }

    /// List all entries in the database
    async fn list(&self) -> Result<BTreeMap<usize, Todo>> {
        let mut conn = self.connection.clone();

        let mut items = BTreeMap::new();

        // get the list of all todo ids
        // if the set doesn't exist, return an empty vector
        // (the [`Default`] implementation for [`Vec`])
        let todo_ids: Vec<String> = conn.smembers("todos").await.unwrap_or_default();

        // iterate over all found ids by ownership (destroying the vector in the process)
        for todo_id_str in todo_ids.into_iter() {
            // find the numeric id of each entry...
            let id = match parse_todo_id(&todo_id_str) {
                Ok(id) => id,
                Err(_) => continue,
            };

            // ... read the corresponding key from Redis...
            let todo: String = conn.get(todo_id_str).await?;

            // ...deserialize the JSON representation into a [`Todo`] object...
            // note: the type of the return value is inferred from the context:
            //       it's passed to `BTreeMap<usize, Todo>::insert`, which means
            //       that its type must be [`Todo`]
            let todo = serde_json::from_str(&todo)?;

            // ... and store it in the result map
            items.insert(id, todo);
        }
        Ok(items)
    }

    /// Mark a todo as done
    async fn mark_done(&self, id: usize) -> Result<bool> {
        let mut conn = self.connection.clone();

        // prepare the id string...
        let todo_id_str = format!("todo:{}", id);

        // ...and fetch the entry from Redis
        let todo_res: RedisResult<String> = conn.get(&todo_id_str).await;

        if let Ok(todo) = todo_res {
            // if it was there, deserialize it, call [`Todo::mark_done`] on it,
            // serialize it again and save to Redis
            let mut todo: Todo = serde_json::from_str(&todo)?;
            todo.mark_done();
            let todo = serde_json::to_string(&todo)?;
            conn.set(todo_id_str, todo).await?;
            Ok(true)
        } else {
            // there was no object under that key
            Ok(false)
        }
    }

    /// Delete a todo entry
    async fn delete(&self, id: usize) -> Result<bool> {
        let mut conn = self.connection.clone();
        let todo_id_str = format!("todo:{}", id);

        conn.srem("todos", &todo_id_str).await?;

        // since Redis operations can return different data types,
        // we need to tell the compiler what type we expect here
        // (the redis library will handle the conversion for us)
        let deleted: usize = conn.del(&todo_id_str).await?;

        Ok(deleted > 0)
    }
}

#[cfg(test)]
mod tests {
    use crate::storage::redis::{parse_todo_id, TodoIdParseError};

    #[test]
    fn test_parse_todo_id() {
        assert_eq!(parse_todo_id("todo:5"), Ok(5));
        assert_eq!(parse_todo_id("tododo:5"), Err(TodoIdParseError));
        assert_eq!(parse_todo_id(""), Err(TodoIdParseError));
        assert_eq!(parse_todo_id("todo:"), Err(TodoIdParseError));
        assert_eq!(parse_todo_id("todo:7:"), Err(TodoIdParseError));
    }
}

#[cfg(all(test, feature = "redis_tests"))]
mod redis_tests {
    use super::*;
    use crate::storage::TodoStatus;

    async fn get_storage() -> RedisStorage {
        let redis_url = std::env::var("REDIS_TEST_URL").unwrap();
        let mut conn = RedisStorage::new(&redis_url).await.unwrap();

        // delete everything from the database
        let _: () = redis::cmd("FLUSHDB")
            .query_async(&mut conn.connection)
            .await
            .unwrap();

        conn
    }

    #[actix_rt::test]
    async fn test_list_empty() {
        let conn = get_storage().await;
        assert!(conn.list().await.unwrap().is_empty());
    }

    #[actix_rt::test]
    async fn test_create() {
        let conn = get_storage().await;
        let id_1 = conn.create(Todo::new("testing".to_string())).await.unwrap();
        assert_eq!(id_1, 1);
        let id_2 = conn
            .create(Todo::new("testing again".to_string()))
            .await
            .unwrap();
        assert_eq!(id_2, 2);
    }

    #[actix_rt::test]
    async fn test_create_and_list() {
        let conn = get_storage().await;
        conn.create(Todo::new("testing".to_string())).await.unwrap();

        let actual: Vec<_> = conn.list().await.unwrap().into_iter().collect();
        let expected = vec![(1, Todo::new("testing".to_string()))];

        assert_eq!(expected, actual);
    }

    #[actix_rt::test]
    async fn test_mark_done() {
        let conn = get_storage().await;
        conn.create(Todo::new("testing".to_string())).await.unwrap();
        conn.mark_done(1).await.unwrap();
        let todos = conn.list().await.unwrap();
        let the_todo = todos.get(&1).unwrap();
        assert_eq!(the_todo.get_status(), TodoStatus::Done);
    }

    #[actix_rt::test]
    async fn test_delete() {
        let conn = get_storage().await;
        conn.create(Todo::new("testing".to_string())).await.unwrap();
        assert_eq!(conn.delete(1).await.unwrap(), true);
        assert_eq!(conn.delete(1).await.unwrap(), false);
    }
}
