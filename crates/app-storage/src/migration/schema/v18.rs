pub(in crate::migration) const MIGRATION_18: &str = r"
CREATE TABLE dialogue_continuity_checkpoints (
    conversation_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    checkpoint_json TEXT NOT NULL CHECK (json_valid(checkpoint_json) AND length(checkpoint_json) BETWEEN 2 AND 2048)
) STRICT;
CREATE TRIGGER dialogue_continuity_scope_insert
BEFORE INSERT ON dialogue_continuity_checkpoints
WHEN NOT EXISTS (SELECT 1 FROM conversations WHERE id = NEW.conversation_id AND project_id = NEW.project_id)
BEGIN SELECT RAISE(ABORT, 'dialogue continuity scope mismatch'); END;
CREATE TRIGGER dialogue_continuity_scope_update
BEFORE UPDATE ON dialogue_continuity_checkpoints
WHEN NOT EXISTS (SELECT 1 FROM conversations WHERE id = NEW.conversation_id AND project_id = NEW.project_id)
BEGIN SELECT RAISE(ABORT, 'dialogue continuity scope mismatch'); END;
INSERT INTO schema_migrations (version, name) VALUES (18, 'dialogue_continuity_checkpoint');
";
