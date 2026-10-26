use crate::{
    AppCore, AppCoreError, AssetId, Conversation, ConversationId, ConversationSummary,
    DEFAULT_CONVERSATION_TITLE, Message, MessageId, MessagePart, MessageRole, ProjectId,
    TaskActivity,
};

impl AppCore {
    /// Lists persisted conversations newest-first without loading message bodies.
    ///
    /// # Errors
    ///
    /// Returns an error when local persistence cannot be read safely.
    pub fn conversations(&self) -> Result<Vec<ConversationSummary>, AppCoreError> {
        let project = self
            .with_storage(|storage| storage.projects())?
            .into_iter()
            .next()
            .ok_or(AppCoreError::ProjectNotFound)?;
        self.project_conversations(&project.id)
    }

    /// Lists chats belonging to one project newest-first.
    ///
    /// # Errors
    ///
    /// Returns an error when the project is unavailable or storage fails.
    pub fn project_conversations(
        &self,
        project_id: &ProjectId,
    ) -> Result<Vec<ConversationSummary>, AppCoreError> {
        if self
            .with_storage(|storage| storage.project(project_id))?
            .is_none()
        {
            return Err(AppCoreError::ProjectNotFound);
        }
        self.with_storage(|storage| storage.conversations(project_id))
    }

    /// Loads one complete persisted conversation by opaque identifier.
    ///
    /// # Errors
    ///
    /// Returns an error when local persistence cannot be decoded safely.
    pub fn conversation(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<Option<Conversation>, AppCoreError> {
        let conversation = self.with_storage(|storage| storage.conversation(conversation_id))?;
        if let Some(conversation) = conversation.as_ref()
            && self.conversation_model(conversation_id)?.is_none()
            && let Some(selection) = self.new_conversation_model(&conversation.project_id)?
        {
            self.with_storage(|storage| {
                storage.set_conversation_model(conversation_id, &selection)
            })?;
        }
        Ok(conversation)
    }

    /// Creates and persists one empty conversation with a fresh opaque identifier.
    ///
    /// # Errors
    ///
    /// Returns an error when the conversation cannot be persisted.
    pub fn create_conversation(&self, created_at: &str) -> Result<Conversation, AppCoreError> {
        let project = self
            .with_storage(|storage| storage.projects())?
            .into_iter()
            .next()
            .ok_or(AppCoreError::ProjectNotFound)?;
        self.create_project_conversation(&project.id, created_at)
    }

    /// Creates and persists an empty chat owned by one project.
    ///
    /// # Errors
    ///
    /// Returns an error when the project is unavailable or persistence fails.
    pub fn create_project_conversation(
        &self,
        project_id: &ProjectId,
        created_at: &str,
    ) -> Result<Conversation, AppCoreError> {
        if self
            .with_storage(|storage| storage.project(project_id))?
            .is_none()
        {
            return Err(AppCoreError::ProjectNotFound);
        }
        let conversation = Conversation {
            id: ConversationId(uuid::Uuid::now_v7().to_string()),
            project_id: project_id.clone(),
            title: DEFAULT_CONVERSATION_TITLE.to_owned(),
            messages: Vec::new(),
            created_at: created_at.to_owned(),
            updated_at: created_at.to_owned(),
        };
        self.with_storage(|storage| storage.save_conversation(&conversation))?;
        if let Some(selection) = self.new_conversation_model(project_id)? {
            self.with_storage(|storage| {
                storage.set_conversation_model(&conversation.id, &selection)
            })?;
        }
        Ok(conversation)
    }

    /// Renames one persisted conversation and updates its recent-history timestamp.
    ///
    /// # Errors
    ///
    /// Returns an error when the title is invalid or persistence fails.
    pub fn rename_conversation(
        &self,
        conversation_id: &ConversationId,
        title: &str,
        updated_at: &str,
    ) -> Result<Option<ConversationSummary>, AppCoreError> {
        self.with_storage(|storage| storage.rename_conversation(conversation_id, title, updated_at))
    }

    /// Persists one user message before its generation is scheduled.
    ///
    /// # Errors
    ///
    /// Returns an error when the conversation or a referenced managed asset is absent,
    /// or when persistence fails.
    pub fn append_user_message(
        &self,
        conversation_id: &ConversationId,
        text: String,
        asset_ids: Vec<AssetId>,
        created_at: &str,
    ) -> Result<Conversation, AppCoreError> {
        let mut conversation = self
            .conversation(conversation_id)?
            .ok_or(AppCoreError::ConversationNotFound)?;
        conversation.messages.push(Message {
            id: MessageId(uuid::Uuid::now_v7().to_string()),
            conversation_id: conversation_id.clone(),
            role: MessageRole::User,
            parts: std::iter::once(MessagePart::Text { text })
                .chain(
                    asset_ids
                        .into_iter()
                        .map(|asset_id| MessagePart::Asset { asset_id }),
                )
                .collect(),
            created_at: created_at.to_owned(),
        });
        created_at.clone_into(&mut conversation.updated_at);
        self.save_conversation(&conversation)?;
        Ok(conversation)
    }

    /// Persists one complete assistant response after successful generation.
    ///
    /// Partial, cancelled and failed responses must not call this method.
    ///
    /// # Errors
    ///
    /// Returns an error when the conversation is absent or persistence fails.
    pub fn append_assistant_message(
        &self,
        conversation_id: &ConversationId,
        reasoning: Option<String>,
        activities: Vec<TaskActivity>,
        text: String,
        created_at: &str,
    ) -> Result<Message, AppCoreError> {
        let mut conversation = self
            .conversation(conversation_id)?
            .ok_or(AppCoreError::ConversationNotFound)?;
        let automatic_title = automatic_conversation_title(&conversation);
        let mut parts = Vec::with_capacity(activities.len() + 2);
        if let Some(reasoning) = reasoning.filter(|reasoning| !reasoning.is_empty()) {
            parts.push(MessagePart::Reasoning { text: reasoning });
        }
        parts.extend(
            activities
                .into_iter()
                .map(|activity| MessagePart::Activity { activity }),
        );
        parts.push(MessagePart::Text { text });
        let message = Message {
            id: MessageId(uuid::Uuid::now_v7().to_string()),
            conversation_id: conversation_id.clone(),
            role: MessageRole::Assistant,
            parts,
            created_at: created_at.to_owned(),
        };
        conversation.messages.push(message.clone());
        if let Some(title) = automatic_title {
            conversation.title = title;
        }
        created_at.clone_into(&mut conversation.updated_at);
        self.save_conversation(&conversation)?;
        Ok(message)
    }
}

pub(super) fn automatic_conversation_title(conversation: &Conversation) -> Option<String> {
    if conversation.title != DEFAULT_CONVERSATION_TITLE
        || conversation.messages.iter().any(|message| {
            message.role == MessageRole::Assistant
                && message
                    .parts
                    .iter()
                    .any(|part| matches!(part, MessagePart::Text { .. }))
                && !message
                    .parts
                    .iter()
                    .any(|part| matches!(part, MessagePart::ToolCall { .. }))
        })
    {
        return None;
    }
    let prompt = conversation.messages.iter().find_map(|message| {
        (message.role == MessageRole::User).then(|| {
            message.parts.iter().find_map(|part| match part {
                MessagePart::Text { text } => Some(text.as_str()),
                _ => None,
            })
        })?
    })?;
    derive_conversation_title(prompt)
}

pub(crate) fn derive_conversation_title(prompt: &str) -> Option<String> {
    const PREFIXES: &[&str] = &[
        "can you ",
        "could you ",
        "would you ",
        "please ",
        "help me ",
        "tell me ",
        "a bit about ",
    ];
    const MAX_TITLE_BYTES: usize = 72;

    let normalized = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut title = normalized.trim();
    loop {
        let lowercase = title.to_lowercase();
        let Some(prefix) = PREFIXES
            .iter()
            .find(|prefix| lowercase.starts_with(**prefix))
        else {
            break;
        };
        title = title[prefix.len()..].trim_start();
    }
    title = title.trim_matches(|character: char| {
        character.is_whitespace()
            || matches!(
                character,
                '"' | '\'' | '`' | '.' | ',' | '!' | '?' | ':' | ';'
            )
    });
    if title.is_empty() {
        title = normalized.trim();
    }
    if title.is_empty() {
        return None;
    }

    let mut end = title.len().min(MAX_TITLE_BYTES);
    while !title.is_char_boundary(end) {
        end -= 1;
    }
    if end < title.len()
        && let Some(word_end) = title[..end].rfind(char::is_whitespace)
        && word_end >= MAX_TITLE_BYTES / 2
    {
        end = word_end;
    }
    let title = title[..end].trim_end();
    let mut characters = title.chars();
    let first = characters.next()?;
    Some(first.to_uppercase().chain(characters).collect())
}
