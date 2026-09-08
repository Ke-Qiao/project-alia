-- One PostgreSQL-owned privacy boundary per user, including users without
-- account rows (the memory domain also serves externally linked identities).
CREATE TABLE memory_privacy_state (
    user_id UUID PRIMARY KEY,
    privacy_epoch BIGINT NOT NULL DEFAULT 0 CHECK (privacy_epoch >= 0),
    source_cutoff_at TIMESTAMPTZ NOT NULL DEFAULT '1970-01-01 00:00:00+00'
);

ALTER TABLE memory_items ADD COLUMN revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0);
ALTER TABLE memory_items DROP CONSTRAINT memory_items_status_check;
ALTER TABLE memory_items ADD CONSTRAINT memory_items_status_check
    CHECK (status IN ('active', 'superseded', 'rejected', 'expired', 'deleted'));
ALTER TABLE memory_embeddings_1536 ADD COLUMN source_revision BIGINT NOT NULL DEFAULT 1;
ALTER TABLE memory_edges ADD COLUMN source_revision BIGINT NOT NULL DEFAULT 1;

-- Source ownership and revision are checked under the same user privacy lock
-- as the derived write. Deletion recursively erases existing derived records.
CREATE TABLE memory_sources (
    memory_id UUID NOT NULL REFERENCES memory_items(id),
    source_id UUID NOT NULL REFERENCES memory_items(id),
    source_revision BIGINT NOT NULL CHECK (source_revision > 0),
    PRIMARY KEY (memory_id, source_id),
    CHECK (memory_id <> source_id)
);
CREATE INDEX memory_sources_dependents ON memory_sources(source_id);

-- Only trusted interaction ingress can supply this timestamp. Legacy records
-- without provenance remain usable as history but are excluded from extraction.
ALTER TABLE chat_messages ADD COLUMN source_received_at TIMESTAMPTZ;

CREATE VIEW memory_live_edges AS
SELECT e.* FROM memory_edges e
JOIN memory_items m ON m.id=e.evidence_memory_id AND m.user_id=e.user_id AND m.tenant_id=e.tenant_id
JOIN memory_nodes src ON src.id=e.src_node_id
JOIN memory_nodes dst ON dst.id=e.dst_node_id
WHERE m.status='active' AND m.revision=e.source_revision
  AND (m.expires_at IS NULL OR m.expires_at>clock_timestamp())
  AND NOT m.sensitive AND m.visibility<>'private_runtime'
  AND (e.valid_to IS NULL OR e.valid_to>clock_timestamp())
  AND (src.properties->>'memory_id' IS NULL OR EXISTS (
    SELECT 1 FROM memory_items s WHERE s.id::text=src.properties->>'memory_id'
      AND s.user_id=e.user_id AND s.status='active' AND NOT s.sensitive
      AND s.visibility<>'private_runtime' AND (s.expires_at IS NULL OR s.expires_at>clock_timestamp())))
  AND (dst.properties->>'memory_id' IS NULL OR EXISTS (
    SELECT 1 FROM memory_items d WHERE d.id::text=dst.properties->>'memory_id'
      AND d.user_id=e.user_id AND d.status='active' AND NOT d.sensitive
      AND d.visibility<>'private_runtime' AND (d.expires_at IS NULL OR d.expires_at>clock_timestamp())));
