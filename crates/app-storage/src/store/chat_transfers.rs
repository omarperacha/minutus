//! Durable admission records contain identity only, never signing authority.
use super::{ApplicationStore, StorageError, params};
use ipc_types::{ConversationId, MessageId};

impl ApplicationStore {
    /// Claims the single transfer attempt for an existing user message.
    /// A false result means already claimed, or an invalid message identity.
    /// # Errors
    /// Returns a storage failure; callers must not proceed without a durable claim.
    pub fn claim_chat_transfer(
        &self,
        conversation: &ConversationId,
        message: &MessageId,
    ) -> Result<bool, StorageError> {
        Ok(self.connection.execute(
            "INSERT INTO chat_transfer_attempts (conversation_id, message_id)
             SELECT ?1, ?2 WHERE EXISTS (
                SELECT 1 FROM messages m WHERE id = ?2 AND conversation_id = ?1 AND role = 'user'
                AND NOT EXISTS (SELECT 1 FROM messages newer WHERE newer.conversation_id = ?1 AND newer.role = 'user' AND newer.position > m.position)
             ) ON CONFLICT DO NOTHING",
            params![conversation.0, message.0],
        )? == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_is_single_use_across_reopen_and_rejects_other_conversations()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("state.sqlite");
        let conversation = ConversationId("chat".into());
        let message = MessageId("user-turn".into());
        {
            let mut store = ApplicationStore::open(&path)?;
            store.connection.execute("INSERT INTO projects (id,name,created_at,updated_at) VALUES ('project','Project','now','now')", [])?;
            store.connection.execute("INSERT INTO conversations (id,title,created_at,updated_at,project_id) SELECT 'chat','Chat','now','now',id FROM projects LIMIT 1", [])?;
            store.connection.execute("INSERT INTO messages (id,conversation_id,position,role,parts_json,created_at) VALUES ('user-turn','chat',0,'user','[]','now')", [])?;
            assert!(!store.claim_chat_transfer(&ConversationId("other".into()), &message)?);
            assert!(store.claim_chat_transfer(&conversation, &message)?);
            assert!(!store.claim_chat_transfer(&conversation, &message)?);
            let history = store
                .conversation(&conversation)?
                .ok_or("missing conversation")?;
            store.save_conversation(&history)?;
            assert!(!store.claim_chat_transfer(&conversation, &message)?);
        }
        let store = ApplicationStore::open(&path)?;
        assert!(!store.claim_chat_transfer(&conversation, &message)?);
        drop(store);
        std::fs::remove_file(path)?;
        Ok(())
    }
    #[test]
    fn parallel_claims_have_one_winner_and_old_turns_cannot_be_replayed()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("parallel.sqlite");
        let first = ApplicationStore::open(&path)?;
        first.connection.execute("INSERT INTO projects (id,name,created_at,updated_at) VALUES ('project','Project','now','now')",[])?;
        first.connection.execute("INSERT INTO conversations (id,title,created_at,updated_at,project_id) VALUES ('chat','Chat','now','now','project')",[])?;
        first.connection.execute("INSERT INTO messages (id,conversation_id,position,role,parts_json,created_at) VALUES ('first','chat',0,'user','[]','now'),('second','chat',1,'user','[]','now'),('assistant','chat',2,'assistant','[]','now')",[])?;
        let conversation = ConversationId("chat".into());
        assert!(!first.claim_chat_transfer(&conversation, &MessageId("first".into()))?);
        assert!(!first.claim_chat_transfer(&conversation, &MessageId("assistant".into()))?);
        let second = ApplicationStore::open(&path)?;
        let barrier = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let barrier = &barrier;
            let left = scope.spawn(move || {
                barrier.wait();
                first.claim_chat_transfer(
                    &ConversationId("chat".into()),
                    &MessageId("second".into()),
                )
            });
            let right = scope.spawn(move || {
                barrier.wait();
                second.claim_chat_transfer(
                    &ConversationId("chat".into()),
                    &MessageId("second".into()),
                )
            });
            (left.join(), right.join())
        });
        let left = results.0.map_err(|_| "claim thread panicked")??;
        let right = results.1.map_err(|_| "claim thread panicked")??;
        assert_ne!(left, right);
        Ok(())
    }
}
