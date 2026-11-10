//! Generic workflow declarations retain historical completion separately from live gates.
pub(in crate::migration) const MIGRATION_25: &str = "
CREATE TABLE contract_workflow_declarations (
    workflow_id INTEGER PRIMARY KEY NOT NULL REFERENCES contract_workflows(id) ON DELETE CASCADE,
    format_version INTEGER NOT NULL CHECK (format_version=2),
    declaration_json TEXT NOT NULL CHECK (length(declaration_json) BETWEEN 1 AND 65536)
) STRICT;
CREATE TABLE contract_workflow_preparations (
    workflow_id INTEGER NOT NULL,
    step_index INTEGER NOT NULL,
    preparation_commitment TEXT NOT NULL CHECK (length(preparation_commitment)=64),
    preparation_json TEXT NOT NULL CHECK (length(preparation_json) BETWEEN 1 AND 16384),
    PRIMARY KEY(workflow_id,step_index),
    FOREIGN KEY(workflow_id,step_index) REFERENCES contract_workflow_steps(workflow_id,step_index) ON DELETE CASCADE
) STRICT;
CREATE TABLE contract_workflow_evidence (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workflow_id INTEGER NOT NULL,
    step_index INTEGER NOT NULL,
    evidence_commitment TEXT NOT NULL CHECK (length(evidence_commitment)=64),
    evidence_json TEXT NOT NULL CHECK (length(evidence_json) BETWEEN 1 AND 16384),
    UNIQUE(workflow_id,step_index,evidence_commitment),
    FOREIGN KEY(workflow_id,step_index) REFERENCES contract_workflow_steps(workflow_id,step_index) ON DELETE CASCADE
) STRICT;
CREATE TRIGGER contract_declaration_immutable BEFORE UPDATE ON contract_workflow_declarations
BEGIN SELECT RAISE(ABORT,'generic workflow declaration is immutable'); END;
CREATE TRIGGER contract_preparation_immutable BEFORE UPDATE ON contract_workflow_preparations
BEGIN SELECT RAISE(ABORT,'generic workflow preparation is immutable'); END;
CREATE TRIGGER contract_evidence_immutable BEFORE UPDATE ON contract_workflow_evidence
BEGIN SELECT RAISE(ABORT,'generic workflow evidence is immutable'); END;
CREATE TRIGGER contract_declaration_excludes_legacy BEFORE INSERT ON contract_workflow_declarations
WHEN EXISTS(SELECT 1 FROM contract_workflow_scopes WHERE workflow_id=NEW.workflow_id)
BEGIN SELECT RAISE(ABORT,'workflow already has a scoped protocol'); END;
CREATE TRIGGER contract_scope_excludes_generic BEFORE INSERT ON contract_workflow_scopes
WHEN EXISTS(SELECT 1 FROM contract_workflow_declarations WHERE workflow_id=NEW.workflow_id)
BEGIN SELECT RAISE(ABORT,'workflow already has a generic protocol'); END;
INSERT INTO schema_migrations(version,name) VALUES (25,'generic_contract_workflow_declarations');
";
