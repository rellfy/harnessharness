use crate::error::Error;
use crate::message::ContentBlock;
use crate::message::Message;
use crate::message::Role;
use crate::session::SessionStore;
use async_trait::async_trait;
use sqlx::Row;
use sqlx::migrate::MigrateError;
use sqlx::migrate::Migrator;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::sqlite::SqliteRow;
use std::path::Path;

const INSERT_MESSAGE: &str = "\
    INSERT INTO message (session_id, role, content) VALUES (?, ?, ?)";
const SELECT_MESSAGES: &str = "\
    SELECT role, content FROM message WHERE session_id = ? ORDER BY id";

static MIGRATOR: Migrator = sqlx::migrate!("src/session/provider/sqlite/migrations");

#[derive(Debug, Clone)]
pub struct Sqlite {
    pool: SqlitePool,
}

impl Sqlite {
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self, Error> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new().connect_with(options).await?;
        Self::from_pool(pool).await
    }

    pub async fn in_memory() -> Result<Self, Error> {
        let options = SqliteConnectOptions::new().in_memory(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        Self::from_pool(pool).await
    }

    pub async fn from_pool(pool: SqlitePool) -> Result<Self, Error> {
        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl SessionStore for Sqlite {
    async fn load(&self, session_id: &str) -> Result<Option<Vec<Message>>, Error> {
        let rows = sqlx::query(SELECT_MESSAGES)
            .bind(session_id)
            .fetch_all(&self.pool)
            .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        let messages = rows
            .iter()
            .map(message_from_row)
            .collect::<Result<Vec<Message>, Error>>()?;
        Ok(Some(messages))
    }

    async fn append(&self, session_id: &str, message: &Message) -> Result<(), Error> {
        let content = serde_json::to_string(&message.content)?;
        sqlx::query(INSERT_MESSAGE)
            .bind(session_id)
            .bind(<&str>::from(message.role))
            .bind(content)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

impl From<sqlx::Error> for Error {
    fn from(error: sqlx::Error) -> Self {
        Error::SessionStore(error.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Error::SessionStore(error.to_string())
    }
}

impl From<MigrateError> for Error {
    fn from(error: MigrateError) -> Self {
        Error::SessionStore(error.to_string())
    }
}

fn message_from_row(row: &SqliteRow) -> Result<Message, Error> {
    let role: String = row.try_get("role")?;
    let content: String = row.try_get("content")?;
    let content: Vec<ContentBlock> = serde_json::from_str(&content)?;
    Ok(Message {
        role: role.parse::<Role>().map_err(|_| Error::UnknownRole(role))?,
        content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ToolResult;
    use crate::message::ToolUse;
    use serde_json::json;

    #[tokio::test]
    async fn appends_and_loads_in_order() {
        let store = Sqlite::in_memory().await.unwrap();
        store.append("a", &Message::user("hi")).await.unwrap();
        store
            .append("a", &Message::assistant("hello"))
            .await
            .unwrap();
        store.append("b", &Message::user("other")).await.unwrap();
        let messages = store.load("a").await.unwrap().unwrap();
        assert_eq!(
            messages,
            vec![Message::user("hi"), Message::assistant("hello")]
        );
    }

    #[tokio::test]
    async fn round_trips_tool_blocks() {
        let store = Sqlite::in_memory().await.unwrap();
        let tool_use = ContentBlock::ToolUse(ToolUse {
            id: "toolu_1".to_string(),
            name: "echo".to_string(),
            input: json!({ "text": "hi" }),
        });
        let messages = vec![
            Message::new(Role::Assistant, vec![ContentBlock::text("ok"), tool_use]),
            Message::tool_results(vec![ToolResult {
                tool_use_id: "toolu_1".to_string(),
                content: "hi".to_string(),
                is_error: false,
            }]),
        ];
        for message in &messages {
            store.append("a", message).await.unwrap();
        }
        assert_eq!(store.load("a").await.unwrap().unwrap(), messages);
    }

    #[tokio::test]
    async fn migrates_plain_text_content_to_blocks() {
        let options = SqliteConnectOptions::new().in_memory(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let legacy_migrator = Migrator {
            migrations: MIGRATOR.migrations[..1].to_vec().into(),
            ..Migrator::DEFAULT
        };
        legacy_migrator.run(&pool).await.unwrap();
        sqlx::query(INSERT_MESSAGE)
            .bind("a")
            .bind("user")
            .bind("it's \"quoted\"")
            .execute(&pool)
            .await
            .unwrap();
        let store = Sqlite::from_pool(pool).await.unwrap();
        let messages = store.load("a").await.unwrap().unwrap();
        assert_eq!(messages, vec![Message::user("it's \"quoted\"")]);
    }

    #[tokio::test]
    async fn loads_none_for_unknown_session() {
        let store = Sqlite::in_memory().await.unwrap();
        assert!(store.load("missing").await.unwrap().is_none());
    }
}
