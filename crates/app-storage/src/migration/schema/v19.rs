//! External model references are independent of managed downloads.
pub(in crate::migration) const MIGRATION_19: &str = "
CREATE TABLE external_model_registrations (
    model_id TEXT PRIMARY KEY NOT NULL,
    record TEXT NOT NULL
) STRICT;
INSERT INTO schema_migrations (version, name) VALUES (19, 'external_model_registrations');
";
