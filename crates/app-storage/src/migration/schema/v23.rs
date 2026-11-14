//! Durable nonce ownership survives monitoring/history deletion and fences old workers.
pub(in crate::migration) const MIGRATION_23: &str = "
CREATE TABLE evm_nonce_claims (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    network TEXT NOT NULL CHECK (network IN ('eip155:1','eip155:11155111','eip155:8453','eip155:84532')),
    sender TEXT NOT NULL CHECK (length(sender)=42 AND substr(sender,1,2)='0x' AND substr(sender,3) NOT GLOB '*[^0-9a-f]*'),
    nonce TEXT NOT NULL CHECK (length(nonce) BETWEEN 1 AND 78 AND nonce NOT GLOB '*[^0-9]*' AND (nonce='0' OR substr(nonce,1,1)!='0')),
    candidate_digest TEXT CHECK (candidate_digest IS NULL OR (length(candidate_digest)=66 AND substr(candidate_digest,1,2)='0x' AND substr(candidate_digest,3) NOT GLOB '*[^0-9a-f]*')),
    owner_token TEXT UNIQUE CHECK (owner_token IS NULL OR (length(owner_token) BETWEEN 1 AND 160 AND owner_token NOT GLOB '*[^A-Za-z0-9-]*')),
    signed_hash TEXT CHECK (signed_hash IS NULL OR length(signed_hash)=66),
    phase TEXT NOT NULL CHECK (phase IN ('reserved','possible_send','retired')),
    CHECK ((candidate_digest IS NULL) = (owner_token IS NULL)),
    CHECK (phase!='reserved' OR owner_token IS NOT NULL),
    CHECK (owner_token IS NULL OR phase!='possible_send' OR signed_hash IS NOT NULL)
) STRICT;
CREATE UNIQUE INDEX evm_nonce_active_key ON evm_nonce_claims(network,sender,nonce) WHERE phase!='retired';
CREATE TABLE evm_nonce_hashes (
    claim_id INTEGER NOT NULL REFERENCES evm_nonce_claims(id),
    transaction_hash TEXT NOT NULL CHECK (length(transaction_hash)=66),
    candidate_digest TEXT NOT NULL CHECK (length(candidate_digest)=66),
    PRIMARY KEY (claim_id,transaction_hash)
) STRICT;

-- Reserved-only legacy sends are fenced below before any old CAS can dispatch.
-- Every possible/known send, including failed receipts and replacements, retains
-- one blocking slot and ALL hashes. Never choose an arbitrary replacement winner.
INSERT INTO evm_nonce_claims(network,sender,nonce,phase)
SELECT DISTINCT t.network,lower(t.sender),t.nonce,'possible_send'
FROM evm_transactions t LEFT JOIN evm_transfer_dispatches d USING(transaction_hash)
WHERE t.state!='pending' OR d.phase IS NULL OR d.phase!='reserved';
INSERT INTO evm_nonce_hashes(claim_id,transaction_hash,candidate_digest)
SELECT c.id,t.transaction_hash,t.candidate_digest FROM evm_transactions t
JOIN evm_nonce_claims c ON c.network=t.network AND c.sender=lower(t.sender) AND c.nonce=t.nonce;

CREATE TRIGGER evm_nonce_hashes_no_delete BEFORE DELETE ON evm_nonce_hashes
BEGIN SELECT RAISE(ABORT,'nonce hash evidence cannot be deleted'); END;
CREATE TRIGGER evm_nonce_hashes_immutable BEFORE UPDATE ON evm_nonce_hashes
BEGIN SELECT RAISE(ABORT,'nonce hash evidence is immutable'); END;
CREATE TRIGGER evm_nonce_no_delete BEFORE DELETE ON evm_nonce_claims BEGIN
    SELECT RAISE(ABORT,'nonce ownership tombstones cannot be deleted');
END;
CREATE TRIGGER evm_nonce_binding_immutable BEFORE UPDATE OF id,network,sender,nonce,candidate_digest,owner_token ON evm_nonce_claims
WHEN NEW.id IS NOT OLD.id OR NEW.network IS NOT OLD.network OR NEW.sender IS NOT OLD.sender OR NEW.nonce IS NOT OLD.nonce OR NEW.candidate_digest IS NOT OLD.candidate_digest OR NEW.owner_token IS NOT OLD.owner_token
BEGIN SELECT RAISE(ABORT,'nonce ownership binding is immutable'); END;
CREATE TRIGGER evm_nonce_no_revive BEFORE UPDATE OF phase ON evm_nonce_claims
WHEN OLD.phase='retired' AND NEW.phase!='retired'
BEGIN SELECT RAISE(ABORT,'retired nonce owner cannot revive'); END;
CREATE TRIGGER evm_nonce_signed_hash_immutable BEFORE UPDATE OF signed_hash ON evm_nonce_claims
WHEN (OLD.signed_hash IS NOT NULL AND NEW.signed_hash IS NOT OLD.signed_hash) OR (OLD.signed_hash IS NULL AND (OLD.phase!='reserved' OR OLD.owner_token IS NULL))
BEGIN SELECT RAISE(ABORT,'signed nonce hash is immutable'); END;
CREATE TRIGGER evm_transaction_nonce_conflict BEFORE INSERT ON evm_transactions
WHEN EXISTS(SELECT 1 FROM evm_nonce_claims c WHERE c.network=NEW.network AND c.sender=lower(NEW.sender) AND c.nonce=NEW.nonce AND c.phase!='retired' AND c.owner_token IS NOT NULL AND (c.candidate_digest IS NOT NEW.candidate_digest OR c.signed_hash IS NOT NEW.transaction_hash))
BEGIN SELECT RAISE(ABORT,'monitoring record conflicts with exact nonce owner'); END;
CREATE TRIGGER evm_transaction_identity_immutable BEFORE UPDATE OF transaction_hash,network,sender,nonce,candidate_digest ON evm_transactions
WHEN NEW.transaction_hash IS NOT OLD.transaction_hash OR NEW.network IS NOT OLD.network OR lower(NEW.sender) IS NOT lower(OLD.sender) OR NEW.nonce IS NOT OLD.nonce OR NEW.candidate_digest IS NOT OLD.candidate_digest
BEGIN SELECT RAISE(ABORT,'transaction identity is immutable'); END;
CREATE TRIGGER evm_transaction_nonce_retention AFTER INSERT ON evm_transactions BEGIN
    INSERT INTO evm_nonce_claims(network,sender,nonce,phase)
    SELECT NEW.network,lower(NEW.sender),NEW.nonce,'possible_send'
    WHERE NOT EXISTS(SELECT 1 FROM evm_nonce_claims WHERE network=NEW.network AND sender=lower(NEW.sender) AND nonce=NEW.nonce AND phase!='retired');
    INSERT INTO evm_nonce_hashes(claim_id,transaction_hash,candidate_digest)
    SELECT id,NEW.transaction_hash,NEW.candidate_digest FROM evm_nonce_claims
    WHERE network=NEW.network AND sender=lower(NEW.sender) AND nonce=NEW.nonce AND phase!='retired';
END;

-- Already-running older binaries do not know about nonce claims. Their legacy
-- SQL insert/update cannot establish dispatch authority after this migration.
CREATE TRIGGER evm_dispatch_requires_nonce BEFORE INSERT ON evm_transfer_dispatches
WHEN NEW.phase='dispatched' OR NOT EXISTS (
    SELECT 1 FROM evm_transactions t JOIN evm_nonce_claims c
    ON c.network=t.network AND c.sender=lower(t.sender) AND c.nonce=t.nonce
    JOIN evm_nonce_hashes h ON h.claim_id=c.id AND h.transaction_hash=t.transaction_hash
    WHERE t.transaction_hash=NEW.transaction_hash AND (
        (NEW.phase='reserved' AND c.phase='reserved' AND c.candidate_digest=t.candidate_digest AND c.owner_token=NEW.reservation_id AND c.signed_hash=t.transaction_hash)
        OR (NEW.phase='acknowledged' AND c.phase='possible_send')
    )
)
BEGIN SELECT RAISE(ABORT,'exact nonce owner required'); END;
CREATE TRIGGER evm_dispatch_identity_immutable BEFORE UPDATE OF transaction_hash,reservation_id ON evm_transfer_dispatches
WHEN NEW.transaction_hash IS NOT OLD.transaction_hash OR NEW.reservation_id IS NOT OLD.reservation_id
BEGIN SELECT RAISE(ABORT,'dispatch owner is immutable'); END;
CREATE TRIGGER evm_dispatch_requires_fence BEFORE UPDATE OF phase ON evm_transfer_dispatches
WHEN NEW.phase!=OLD.phase AND NOT (
    (OLD.phase='reserved' AND NEW.phase='dispatched' AND EXISTS (
        SELECT 1 FROM evm_transactions t JOIN evm_nonce_claims c
        ON c.network=t.network AND c.sender=lower(t.sender) AND c.nonce=t.nonce
        JOIN evm_nonce_hashes h ON h.claim_id=c.id AND h.transaction_hash=t.transaction_hash
        WHERE t.transaction_hash=NEW.transaction_hash AND c.phase='possible_send'
        AND c.candidate_digest=t.candidate_digest AND h.candidate_digest=t.candidate_digest
        AND c.signed_hash=t.transaction_hash AND c.owner_token=NEW.reservation_id
    )) OR (OLD.phase='dispatched' AND NEW.phase='acknowledged' AND EXISTS (
        SELECT 1 FROM evm_transactions t JOIN evm_nonce_claims c
        ON c.network=t.network AND c.sender=lower(t.sender) AND c.nonce=t.nonce
        WHERE t.transaction_hash=NEW.transaction_hash AND c.phase='possible_send'
    ))
)
BEGIN SELECT RAISE(ABORT,'exact nonce dispatch fence required'); END;
INSERT INTO schema_migrations(version,name) VALUES (23,'durable_evm_nonce_ownership');
";
