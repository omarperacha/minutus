pub(in crate::migration) const MIGRATION_1: &str = r"
CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL
) STRICT;

CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL CHECK (json_valid(value_json)),
    updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE conversations (
    id TEXT PRIMARY KEY CHECK (length(id) > 0),
    title TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE assets (
    id TEXT PRIMARY KEY CHECK (length(id) > 0),
    storage_key TEXT NOT NULL UNIQUE CHECK (length(storage_key) > 0),
    media_type TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    byte_size INTEGER NOT NULL CHECK (byte_size >= 0),
    width INTEGER CHECK (width IS NULL OR width > 0),
    height INTEGER CHECK (height IS NULL OR height > 0),
    imported_at TEXT NOT NULL,
    CHECK ((width IS NULL) = (height IS NULL))
) STRICT;

CREATE TABLE messages (
    id TEXT PRIMARY KEY CHECK (length(id) > 0),
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'tool')),
    parts_json TEXT NOT NULL CHECK (json_valid(parts_json)),
    created_at TEXT NOT NULL,
    UNIQUE (conversation_id, position)
) STRICT;

CREATE TABLE message_assets (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    asset_id TEXT NOT NULL REFERENCES assets(id) ON DELETE RESTRICT,
    PRIMARY KEY (message_id, asset_id)
) STRICT;

CREATE TABLE workspace_grants (
    id TEXT PRIMARY KEY CHECK (length(id) > 0),
    display_name TEXT NOT NULL,
    canonical_root TEXT NOT NULL,
    bookmark_data BLOB NOT NULL,
    created_at TEXT NOT NULL,
    revoked_at TEXT,
    CHECK (
        (revoked_at IS NULL AND length(bookmark_data) > 0) OR
        (revoked_at IS NOT NULL AND length(bookmark_data) = 0)
    )
) STRICT;

CREATE TABLE models (
    id TEXT PRIMARY KEY CHECK (length(id) > 0),
    display_name TEXT NOT NULL,
    format TEXT NOT NULL,
    local_path TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    byte_size INTEGER NOT NULL CHECK (byte_size >= 0),
    imported_at TEXT NOT NULL
) STRICT;

CREATE TABLE public_wallets (
    id TEXT PRIMARY KEY CHECK (length(id) > 0),
    address TEXT NOT NULL,
    network TEXT NOT NULL,
    availability TEXT NOT NULL CHECK (availability IN ('locked', 'ready')),
    updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE audit_events (
    id TEXT PRIMARY KEY CHECK (length(id) > 0),
    event_kind TEXT NOT NULL CHECK (event_kind IN (
        'application_opened',
        'workspace_granted',
        'workspace_revoked',
        'model_registered',
        'conversation_deleted'
    )),
    outcome TEXT NOT NULL CHECK (outcome IN ('succeeded', 'denied', 'failed')),
    subject_id TEXT,
    occurred_at TEXT NOT NULL
) STRICT;

INSERT INTO schema_migrations (version, name)
VALUES (1, 'initial_application_persistence');
";

pub(in crate::migration) const MIGRATION_2: &str = r"
ALTER TABLE assets ADD COLUMN original_storage_key TEXT NOT NULL DEFAULT '';
ALTER TABLE assets ADD COLUMN original_media_type TEXT NOT NULL DEFAULT '';
ALTER TABLE assets ADD COLUMN original_content_digest TEXT NOT NULL DEFAULT '';
ALTER TABLE assets ADD COLUMN original_byte_size INTEGER NOT NULL DEFAULT 0;
ALTER TABLE assets ADD COLUMN original_width INTEGER NOT NULL DEFAULT 1;
ALTER TABLE assets ADD COLUMN original_height INTEGER NOT NULL DEFAULT 1;

UPDATE assets SET
    original_storage_key = storage_key,
    original_media_type = media_type,
    original_content_digest = content_digest,
    original_byte_size = byte_size,
    original_width = COALESCE(width, 1),
    original_height = COALESCE(height, 1);

CREATE UNIQUE INDEX assets_original_storage_key_unique
ON assets(original_storage_key);

INSERT INTO schema_migrations (version, name)
VALUES (2, 'separate_original_and_inference_assets');
";

pub(in crate::migration) const MIGRATION_3: &str = r"
CREATE TABLE model_companions (
    model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    artifact_id TEXT NOT NULL CHECK (length(artifact_id) > 0),
    kind TEXT NOT NULL CHECK (kind IN ('visionProjector', 'draftModel')),
    required INTEGER NOT NULL CHECK (required IN (0, 1)),
    local_path TEXT NOT NULL UNIQUE,
    content_digest TEXT NOT NULL,
    byte_size INTEGER NOT NULL CHECK (byte_size > 0),
    compatibility_version TEXT NOT NULL CHECK (length(compatibility_version) > 0),
    memory_requirement_bytes INTEGER NOT NULL CHECK (memory_requirement_bytes > 0),
    installed_at TEXT NOT NULL,
    PRIMARY KEY (model_id, artifact_id)
) STRICT;

INSERT INTO schema_migrations (version, name)
VALUES (3, 'verified_model_companion_artifacts');
";

pub(in crate::migration) const MIGRATION_4: &str = r"
ALTER TABLE workspace_grants ADD COLUMN permission_level TEXT NOT NULL
    DEFAULT 'read_only'
    CHECK (permission_level IN ('read_only', 'read_and_propose'));

INSERT INTO schema_migrations (version, name)
VALUES (4, 'workspace_permission_levels');
";

pub(in crate::migration) const MIGRATION_5: &str = r"
ALTER TABLE workspace_grants RENAME TO workspace_grants_legacy;

CREATE TABLE workspace_grants (
    id TEXT PRIMARY KEY CHECK (length(id) > 0),
    display_name TEXT NOT NULL,
    canonical_root TEXT NOT NULL,
    bookmark_data BLOB NOT NULL,
    created_at TEXT NOT NULL,
    revoked_at TEXT,
    permission_level TEXT NOT NULL DEFAULT 'read_only'
        CHECK (permission_level IN ('read_only', 'read_and_write')),
    CHECK (
        (revoked_at IS NULL AND length(bookmark_data) > 0) OR
        (revoked_at IS NOT NULL AND length(bookmark_data) = 0)
    )
) STRICT;

INSERT INTO workspace_grants
    (id, display_name, canonical_root, bookmark_data, created_at, revoked_at, permission_level)
SELECT
    id,
    display_name,
    canonical_root,
    bookmark_data,
    created_at,
    revoked_at,
    CASE permission_level
        WHEN 'read_and_propose' THEN 'read_only'
        ELSE permission_level
    END
FROM workspace_grants_legacy;

DROP TABLE workspace_grants_legacy;

CREATE TABLE projects (
    id TEXT PRIMARY KEY CHECK (length(id) > 0),
    name TEXT NOT NULL CHECK (length(name) > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

ALTER TABLE conversations ADD COLUMN project_id TEXT REFERENCES projects(id) ON DELETE CASCADE;

INSERT INTO projects (id, name, created_at, updated_at)
SELECT
    'project-default',
    COALESCE(
        (SELECT display_name FROM workspace_grants
         WHERE revoked_at IS NULL ORDER BY created_at, id LIMIT 1),
        'My project'
    ),
    COALESCE(
        (SELECT MIN(created_at) FROM conversations),
        (SELECT MIN(created_at) FROM workspace_grants),
        '1970-01-01T00:00:00Z'
    ),
    COALESCE(
        (SELECT MAX(updated_at) FROM conversations),
        (SELECT MAX(created_at) FROM workspace_grants),
        '1970-01-01T00:00:00Z'
    )
WHERE EXISTS (SELECT 1 FROM conversations)
   OR EXISTS (SELECT 1 FROM workspace_grants WHERE revoked_at IS NULL);

UPDATE conversations SET project_id = 'project-default';

CREATE TRIGGER conversations_project_required_insert
BEFORE INSERT ON conversations
WHEN NEW.project_id IS NULL
BEGIN
    SELECT RAISE(ABORT, 'conversation project is required');
END;

CREATE TRIGGER conversations_project_required_update
BEFORE UPDATE OF project_id ON conversations
WHEN NEW.project_id IS NULL
BEGIN
    SELECT RAISE(ABORT, 'conversation project is required');
END;

CREATE TABLE project_workspaces (
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    workspace_grant_id TEXT NOT NULL REFERENCES workspace_grants(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('primary', 'additional')),
    added_at TEXT NOT NULL,
    PRIMARY KEY (project_id, workspace_grant_id)
) STRICT;

CREATE UNIQUE INDEX project_workspaces_one_primary
ON project_workspaces(project_id) WHERE role = 'primary';

INSERT INTO project_workspaces (project_id, workspace_grant_id, role, added_at)
SELECT
    'project-default',
    id,
    CASE
        WHEN id = (SELECT id FROM workspace_grants
                   WHERE revoked_at IS NULL ORDER BY created_at, id LIMIT 1)
        THEN 'primary'
        ELSE 'additional'
    END,
    created_at
FROM workspace_grants
WHERE revoked_at IS NULL;

INSERT INTO schema_migrations (version, name)
VALUES (5, 'project_owned_conversations_and_workspaces');
";

pub(in crate::migration) const MIGRATION_6: &str = r"
ALTER TABLE audit_events RENAME TO audit_events_legacy;

CREATE TABLE audit_events (
    id TEXT PRIMARY KEY CHECK (length(id) > 0),
    event_kind TEXT NOT NULL CHECK (event_kind IN (
        'application_opened',
        'project_created',
        'workspace_granted',
        'workspace_permission_changed',
        'workspace_revoked',
        'filesystem_tool',
        'model_registered',
        'conversation_deleted'
    )),
    outcome TEXT NOT NULL CHECK (outcome IN ('succeeded', 'denied', 'failed')),
    subject_id TEXT,
    operation_id TEXT CHECK (operation_id IS NULL OR length(operation_id) > 0),
    tool_name TEXT CHECK (tool_name IS NULL OR tool_name IN (
        'filesystem.list',
        'filesystem.stat',
        'filesystem.read_text',
        'filesystem.search',
        'filesystem.scan',
        'filesystem.write_text',
        'filesystem.replace_text',
        'filesystem.undo_text',
        'filesystem.undo_available'
    )),
    workspace_grant_id TEXT CHECK (
        workspace_grant_id IS NULL OR length(workspace_grant_id) > 0
    ),
    relative_path TEXT CHECK (
        relative_path IS NULL OR (
            length(relative_path) BETWEEN 1 AND 1024 AND
            substr(relative_path, 1, 1) NOT IN ('/', char(92)) AND
            instr(relative_path, char(0)) = 0
        )
    ),
    permission_decision TEXT NOT NULL CHECK (
        permission_decision IN ('allowed', 'denied', 'not_applicable')
    ),
    result_count INTEGER CHECK (result_count IS NULL OR result_count >= 0),
    truncated INTEGER CHECK (truncated IS NULL OR truncated IN (0, 1)),
    occurred_at TEXT NOT NULL,
    CHECK (
        event_kind != 'filesystem_tool' OR
        (tool_name IS NOT NULL AND workspace_grant_id IS NOT NULL)
    )
) STRICT;

INSERT INTO audit_events (
    id, event_kind, outcome, subject_id, operation_id, tool_name,
    workspace_grant_id, relative_path, permission_decision,
    result_count, truncated, occurred_at
)
SELECT
    id, event_kind, outcome, subject_id, NULL, NULL,
    NULL, NULL, 'not_applicable', NULL, NULL, occurred_at
FROM audit_events_legacy;

DROP TABLE audit_events_legacy;

CREATE INDEX audit_events_occurred_at ON audit_events(occurred_at, id);
CREATE INDEX audit_events_workspace_grant ON audit_events(workspace_grant_id, occurred_at);

INSERT INTO schema_migrations (version, name)
VALUES (6, 'bounded_filesystem_audit_events');
";

pub(in crate::migration) const MIGRATION_7: &str = r"
ALTER TABLE audit_events RENAME TO audit_events_legacy;

CREATE TABLE audit_events (
    id TEXT PRIMARY KEY CHECK (length(id) > 0),
    event_kind TEXT NOT NULL CHECK (event_kind IN (
        'application_opened',
        'project_created',
        'workspace_granted',
        'workspace_permission_changed',
        'workspace_revoked',
        'filesystem_tool',
        'model_registered',
        'conversation_deleted'
    )),
    outcome TEXT NOT NULL CHECK (outcome IN ('succeeded', 'denied', 'failed')),
    subject_id TEXT,
    operation_id TEXT CHECK (operation_id IS NULL OR length(operation_id) > 0),
    tool_name TEXT CHECK (tool_name IS NULL OR tool_name IN (
        'filesystem.list',
        'filesystem.stat',
        'filesystem.read_text',
        'filesystem.inspect_image',
        'filesystem.search',
        'filesystem.scan',
        'filesystem.write_text',
        'filesystem.replace_text',
        'filesystem.undo_text',
        'filesystem.undo_available'
    )),
    workspace_grant_id TEXT CHECK (
        workspace_grant_id IS NULL OR length(workspace_grant_id) > 0
    ),
    relative_path TEXT CHECK (
        relative_path IS NULL OR (
            length(relative_path) BETWEEN 1 AND 1024 AND
            substr(relative_path, 1, 1) NOT IN ('/', char(92)) AND
            instr(relative_path, char(0)) = 0
        )
    ),
    permission_decision TEXT NOT NULL CHECK (
        permission_decision IN ('allowed', 'denied', 'not_applicable')
    ),
    result_count INTEGER CHECK (result_count IS NULL OR result_count >= 0),
    truncated INTEGER CHECK (truncated IS NULL OR truncated IN (0, 1)),
    occurred_at TEXT NOT NULL,
    CHECK (
        event_kind != 'filesystem_tool' OR
        (tool_name IS NOT NULL AND workspace_grant_id IS NOT NULL)
    )
) STRICT;

INSERT INTO audit_events (
    id, event_kind, outcome, subject_id, operation_id, tool_name,
    workspace_grant_id, relative_path, permission_decision,
    result_count, truncated, occurred_at
)
SELECT
    id, event_kind, outcome, subject_id, operation_id, tool_name,
    workspace_grant_id, relative_path, permission_decision,
    result_count, truncated, occurred_at
FROM audit_events_legacy;

DROP TABLE audit_events_legacy;

CREATE INDEX audit_events_occurred_at ON audit_events(occurred_at, id);
CREATE INDEX audit_events_workspace_grant ON audit_events(workspace_grant_id, occurred_at);

INSERT INTO schema_migrations (version, name)
VALUES (7, 'workspace_image_inspection_audit');
";

pub(in crate::migration) const MIGRATION_8: &str = r"
CREATE TABLE execution_ledgers (
    run_id TEXT PRIMARY KEY CHECK (length(run_id) BETWEEN 1 AND 160),
    conversation_id TEXT NOT NULL UNIQUE
        REFERENCES conversations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    snapshot_json TEXT NOT NULL CHECK (
        json_valid(snapshot_json) AND length(snapshot_json) BETWEEN 2 AND 1048576
    ),
    updated_at TEXT NOT NULL CHECK (length(updated_at) > 0)
) STRICT;

CREATE TRIGGER execution_ledgers_project_scope_insert
BEFORE INSERT ON execution_ledgers
WHEN NOT EXISTS (
    SELECT 1 FROM conversations
    WHERE conversations.id = NEW.conversation_id
      AND conversations.project_id = NEW.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'execution ledger project scope mismatch');
END;

CREATE TRIGGER execution_ledgers_project_scope_update
BEFORE UPDATE OF conversation_id, project_id ON execution_ledgers
WHEN NOT EXISTS (
    SELECT 1 FROM conversations
    WHERE conversations.id = NEW.conversation_id
      AND conversations.project_id = NEW.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'execution ledger project scope mismatch');
END;

CREATE TRIGGER conversations_preserve_execution_ledger_scope
BEFORE UPDATE OF project_id ON conversations
WHEN EXISTS (
    SELECT 1 FROM execution_ledgers
    WHERE execution_ledgers.conversation_id = OLD.id
      AND execution_ledgers.project_id != NEW.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'conversation execution ledger scope mismatch');
END;

CREATE INDEX execution_ledgers_project ON execution_ledgers(project_id, updated_at);

INSERT INTO schema_migrations (version, name)
VALUES (8, 'trusted_agent_execution_ledgers');
";

pub(in crate::migration) const MIGRATION_9: &str = r"
CREATE TABLE compact_tool_results (
    id TEXT PRIMARY KEY CHECK (length(id) BETWEEN 1 AND 160),
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    tool_call_id TEXT NOT NULL CHECK (length(tool_call_id) BETWEEN 1 AND 160),
    tool_name TEXT NOT NULL CHECK (length(tool_name) BETWEEN 1 AND 160),
    status TEXT NOT NULL CHECK (status IN ('completed', 'failed', 'cancelled')),
    record_json TEXT NOT NULL CHECK (
        json_valid(record_json) AND length(record_json) BETWEEN 2 AND 17825792
    ),
    created_at TEXT NOT NULL CHECK (length(created_at) BETWEEN 1 AND 64),
    expires_at TEXT NOT NULL CHECK (length(expires_at) BETWEEN 1 AND 64)
) STRICT;

CREATE TABLE compact_result_assets (
    compact_result_id TEXT NOT NULL
        REFERENCES compact_tool_results(id) ON DELETE CASCADE,
    asset_id TEXT NOT NULL REFERENCES assets(id) ON DELETE RESTRICT,
    PRIMARY KEY (compact_result_id, asset_id)
) STRICT;

CREATE TRIGGER compact_tool_results_project_scope_insert
BEFORE INSERT ON compact_tool_results
WHEN NOT EXISTS (
    SELECT 1 FROM conversations
    WHERE conversations.id = NEW.conversation_id
      AND conversations.project_id = NEW.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'compact result project scope mismatch');
END;

CREATE TRIGGER compact_tool_results_project_scope_update
BEFORE UPDATE OF conversation_id, project_id ON compact_tool_results
WHEN NOT EXISTS (
    SELECT 1 FROM conversations
    WHERE conversations.id = NEW.conversation_id
      AND conversations.project_id = NEW.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'compact result project scope mismatch');
END;

CREATE INDEX compact_tool_results_scope
ON compact_tool_results(conversation_id, project_id, expires_at);
CREATE INDEX compact_result_assets_asset ON compact_result_assets(asset_id);

INSERT INTO schema_migrations (version, name)
VALUES (9, 'compact_tool_result_persistence');
";
