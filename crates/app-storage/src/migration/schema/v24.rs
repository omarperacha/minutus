//! Immutable workflow intent and fresh exact candidate/observer context.
pub(in crate::migration) const MIGRATION_24: &str = "
ALTER TABLE contract_workflows ADD COLUMN scope_format INTEGER NOT NULL DEFAULT 0 CHECK (scope_format IN (0,1));
CREATE TRIGGER contract_scope_format_immutable BEFORE UPDATE OF scope_format ON contract_workflows
WHEN NEW.scope_format IS NOT OLD.scope_format
BEGIN SELECT RAISE(ABORT,'workflow scope format is immutable'); END;
CREATE TABLE contract_workflow_scopes (
    workflow_id INTEGER PRIMARY KEY NOT NULL REFERENCES contract_workflows(id) ON DELETE CASCADE,
    format_version INTEGER NOT NULL CHECK (format_version=1),
    scope_json TEXT NOT NULL CHECK (length(scope_json) BETWEEN 1 AND 65536)
) STRICT;
CREATE TABLE contract_workflow_candidates (
    workflow_id INTEGER NOT NULL,
    step_index INTEGER NOT NULL,
    candidate_commitment TEXT NOT NULL CHECK (length(candidate_commitment)=64),
    context_json TEXT NOT NULL CHECK (length(context_json) BETWEEN 1 AND 16384),
    PRIMARY KEY(workflow_id,step_index),
    FOREIGN KEY(workflow_id,step_index) REFERENCES contract_workflow_steps(workflow_id,step_index) ON DELETE CASCADE
) STRICT;
CREATE TABLE contract_workflow_observations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workflow_id INTEGER NOT NULL,
    step_index INTEGER NOT NULL,
    evidence_commitment TEXT NOT NULL CHECK (length(evidence_commitment)=64),
    observation_json TEXT NOT NULL CHECK (length(observation_json) BETWEEN 1 AND 16384),
    UNIQUE(workflow_id,step_index,evidence_commitment),
    FOREIGN KEY(workflow_id,step_index) REFERENCES contract_workflow_steps(workflow_id,step_index) ON DELETE CASCADE
) STRICT;
CREATE TRIGGER contract_scope_immutable BEFORE UPDATE ON contract_workflow_scopes
BEGIN SELECT RAISE(ABORT,'workflow semantic scope is immutable'); END;
CREATE TRIGGER contract_candidate_immutable BEFORE UPDATE ON contract_workflow_candidates
BEGIN SELECT RAISE(ABORT,'workflow candidate binding is immutable'); END;
CREATE TRIGGER contract_observation_immutable BEFORE UPDATE ON contract_workflow_observations
BEGIN SELECT RAISE(ABORT,'workflow observation is immutable'); END;
INSERT INTO schema_migrations(version,name) VALUES (24,'scoped_contract_workflow_context');
";
