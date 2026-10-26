use super::{
    ApplicationStore, AssetId, Conversation, ConversationId, ConversationSummary,
    MAX_CONVERSATION_TITLE_BYTES, Message, MessagePart, MessageRole, ModelSelection,
    OptionalExtension, ProjectId, StorageError, TransactionBehavior, params,
};

impl ApplicationStore {
    /// Atomically replaces a conversation and its ordered message metadata.
    ///
    /// Asset parts are persisted as stable IDs and foreign-keyed references;
    /// asset bytes and base64 data are never stored in the conversation rows.
    ///
    /// # Errors
    ///
    /// Returns an error when identifiers are inconsistent, serialization
    /// fails, a referenced asset is absent, or the transaction fails.
    pub fn save_conversation(&mut self, conversation: &Conversation) -> Result<(), StorageError> {
        validate_conversation(conversation)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO conversations (id, title, created_at, updated_at, project_id)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at,
                project_id = excluded.project_id",
            params![
                conversation.id.0,
                conversation.title,
                conversation.created_at,
                conversation.updated_at,
                conversation.project_id.0
            ],
        )?;
        transaction.execute(
            "DELETE FROM messages WHERE conversation_id = ?1",
            [&conversation.id.0],
        )?;

        for (position, message) in conversation.messages.iter().enumerate() {
            let position = i64::try_from(position)
                .map_err(|_| StorageError::InvalidRecord("too many conversation messages"))?;
            let parts_json = serde_json::to_string(&message.parts)?;
            transaction.execute(
                "INSERT INTO messages
                    (id, conversation_id, position, role, parts_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    message.id.0,
                    message.conversation_id.0,
                    position,
                    role_name(&message.role),
                    parts_json,
                    message.created_at
                ],
            )?;
            for asset_id in message.parts.iter().flat_map(message_part_asset_ids) {
                transaction.execute(
                    "INSERT OR IGNORE INTO message_assets (message_id, asset_id)
                         VALUES (?1, ?2)",
                    params![message.id.0, asset_id.0],
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// Loads a complete conversation in its persisted message order.
    ///
    /// # Errors
    ///
    /// Returns an error when stored roles or message parts are malformed, or
    /// the database read fails.
    pub fn conversation(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<Option<Conversation>, StorageError> {
        let header = self
            .connection
            .query_row(
                "SELECT project_id, title, created_at, updated_at
                 FROM conversations WHERE id = ?1",
                [&conversation_id.0],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((project_id, title, created_at, updated_at)) = header else {
            return Ok(None);
        };

        let mut statement = self.connection.prepare(
            "SELECT id, role, parts_json, created_at
             FROM messages WHERE conversation_id = ?1 ORDER BY position",
        )?;
        let rows = statement.query_map([&conversation_id.0], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut messages = Vec::new();
        for row in rows {
            let (id, role, parts_json, message_created_at) = row?;
            messages.push(Message {
                id: ipc_types::MessageId(id),
                conversation_id: conversation_id.clone(),
                role: parse_role(&role)?,
                parts: serde_json::from_str(&parts_json)?,
                created_at: message_created_at,
            });
        }

        Ok(Some(Conversation {
            id: conversation_id.clone(),
            project_id: ProjectId(project_id),
            title,
            messages,
            created_at,
            updated_at,
        }))
    }

    /// Loads the exact model release bound to a conversation.
    ///
    /// # Errors
    ///
    /// Returns an error for an incomplete stored identity or database failure.
    pub fn conversation_model(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<Option<ModelSelection>, StorageError> {
        self.connection
            .query_row(
                "SELECT model_id, model_version FROM conversations WHERE id = ?1",
                [&conversation_id.0],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .optional()?
            .map(|(id, version)| match (id, version) {
                (Some(model_id), Some(model_version)) => Ok(Some(ModelSelection {
                    model_id,
                    model_version,
                })),
                (None, None) => Ok(None),
                _ => Err(StorageError::InvalidRecord(
                    "conversation model identity is incomplete",
                )),
            })
            .transpose()
            .map(Option::flatten)
    }

    /// Atomically binds a conversation to one exact catalogue release.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid identity or transaction failure.
    pub fn set_conversation_model(
        &mut self,
        conversation_id: &ConversationId,
        selection: &ModelSelection,
    ) -> Result<bool, StorageError> {
        if selection.model_id.is_empty() || selection.model_version.is_empty() {
            return Err(StorageError::InvalidRecord(
                "model identity must not be empty",
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE conversations SET model_id = ?1, model_version = ?2
             WHERE id = ?3 AND (model_id IS NOT ?1 OR model_version IS NOT ?2)",
            params![
                selection.model_id,
                selection.model_version,
                conversation_id.0
            ],
        )?;
        if changed != 0 {
            transaction.execute(
                "DELETE FROM semantic_compaction_checkpoints WHERE conversation_id = ?1",
                [&conversation_id.0],
            )?;
            transaction.execute(
                "DELETE FROM rich_evidence_checkpoints WHERE conversation_id = ?1",
                [&conversation_id.0],
            )?;
        }
        transaction.commit()?;
        Ok(changed != 0)
    }

    /// Lists conversations newest-first without loading message bodies.
    ///
    /// # Errors
    ///
    /// Returns an error when the database query or record decoding fails.
    pub fn conversations(
        &self,
        project_id: &ProjectId,
    ) -> Result<Vec<ConversationSummary>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, title, updated_at FROM conversations
             WHERE project_id = ?1 ORDER BY updated_at DESC, id ASC",
        )?;
        let rows = statement.query_map([&project_id.0], |row| {
            Ok(ConversationSummary {
                id: ConversationId(row.get(0)?),
                project_id: project_id.clone(),
                title: row.get(1)?,
                updated_at: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Renames a conversation and moves it to the top of the recent-history order.
    ///
    /// # Errors
    ///
    /// Returns an error when the title or timestamp is invalid, or the update fails.
    pub fn rename_conversation(
        &self,
        conversation_id: &ConversationId,
        title: &str,
        updated_at: &str,
    ) -> Result<Option<ConversationSummary>, StorageError> {
        validate_conversation_title(title)?;
        if conversation_id.0.is_empty() || updated_at.is_empty() {
            return Err(StorageError::InvalidRecord(
                "conversation identifiers and timestamps must not be empty",
            ));
        }
        let changed = self.connection.execute(
            "UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3",
            params![title, updated_at, conversation_id.0],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        let project_id = self.connection.query_row(
            "SELECT project_id FROM conversations WHERE id = ?1",
            [&conversation_id.0],
            |row| row.get::<_, String>(0).map(ProjectId),
        )?;
        Ok(Some(ConversationSummary {
            id: conversation_id.clone(),
            project_id,
            title: title.to_owned(),
            updated_at: updated_at.to_owned(),
        }))
    }
}

fn message_part_asset_ids(part: &MessagePart) -> Vec<&AssetId> {
    match part {
        MessagePart::Asset { asset_id } => vec![asset_id],
        MessagePart::ToolResult { result } => match &result.outcome {
            ipc_types::ToolOutcome::Completed { content } => content
                .iter()
                .filter_map(|part| match part {
                    ipc_types::ToolResultPart::Asset { asset_id } => Some(asset_id),
                    ipc_types::ToolResultPart::Text { .. }
                    | ipc_types::ToolResultPart::Json { .. } => None,
                })
                .collect(),
            ipc_types::ToolOutcome::Failed { .. } => Vec::new(),
        },
        MessagePart::Activity { .. }
        | MessagePart::Reasoning { .. }
        | MessagePart::Text { .. }
        | MessagePart::ToolCall { .. }
        | MessagePart::FilesystemUndo { .. }
        | MessagePart::FilesystemWriteRecord { .. } => Vec::new(),
    }
}

fn validate_conversation(conversation: &Conversation) -> Result<(), StorageError> {
    if conversation.id.0.is_empty()
        || conversation.project_id.0.is_empty()
        || conversation.created_at.is_empty()
        || conversation.updated_at.is_empty()
    {
        return Err(StorageError::InvalidRecord(
            "conversation identifiers and timestamps must not be empty",
        ));
    }
    validate_conversation_title(&conversation.title)?;
    if conversation
        .messages
        .iter()
        .any(|message| message.conversation_id != conversation.id || message.id.0.is_empty())
    {
        return Err(StorageError::InvalidRecord(
            "message identifiers must be non-empty and match their conversation",
        ));
    }
    Ok(())
}

fn validate_conversation_title(title: &str) -> Result<(), StorageError> {
    if title.is_empty() || title.trim() != title || title.len() > MAX_CONVERSATION_TITLE_BYTES {
        return Err(StorageError::InvalidRecord(
            "conversation title must be non-empty, trimmed and within the size limit",
        ));
    }
    Ok(())
}

const fn role_name(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}
fn parse_role(role: &str) -> Result<MessageRole, StorageError> {
    match role {
        "user" => Ok(MessageRole::User),
        "assistant" => Ok(MessageRole::Assistant),
        "tool" => Ok(MessageRole::Tool),
        _ => Err(StorageError::CorruptDatabase),
    }
}
