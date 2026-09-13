-- Text shares dispatch ownership and recovery, never audio frame evidence.
ALTER TABLE deliveries
    ADD COLUMN media_kind TEXT NOT NULL DEFAULT 'audio' CHECK (media_kind IN ('audio','text')),
    ADD COLUMN text_identity JSONB,
    ADD COLUMN text_offered_utf8_bytes BIGINT NOT NULL DEFAULT 0 CHECK (text_offered_utf8_bytes BETWEEN 0 AND 262144),
    ADD COLUMN text_offered_chunks BIGINT NOT NULL DEFAULT 0 CHECK (text_offered_chunks BETWEEN 0 AND 1024),
    ADD COLUMN text_end_offered BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN text_progress JSONB;

-- The previous anonymous checks described only audio submission/presentation.
DO $$ DECLARE c RECORD; BEGIN
    FOR c IN SELECT conname FROM pg_constraint WHERE conrelid='deliveries'::regclass AND contype='c'
        AND (pg_get_constraintdef(oid) LIKE '%status <> ''submitted''%' OR pg_get_constraintdef(oid) LIKE '%status <> ''completed''%')
    LOOP EXECUTE format('ALTER TABLE deliveries DROP CONSTRAINT %I', c.conname); END LOOP;
END $$;

ALTER TABLE deliveries ADD CONSTRAINT deliveries_media_evidence CHECK (
    (media_kind='audio' AND text_identity IS NULL AND text_offered_utf8_bytes=0 AND text_offered_chunks=0 AND NOT text_end_offered AND text_progress IS NULL)
    OR (media_kind='text' AND identity IS NULL AND playback_id IS NULL AND source_sample_rate IS NULL AND source_channels IS NULL
        AND offered_frames=0 AND accepted_frames=0 AND submitted_frames IS NULL AND presented_frames IS NULL AND NOT end_accepted AND progress IS NULL)
);
ALTER TABLE deliveries ADD CONSTRAINT deliveries_submitted_evidence CHECK (status<>'submitted' OR
    (media_kind='audio' AND end_accepted AND submitted_frames IS NOT NULL AND submitted_frames=offered_frames) OR
    (media_kind='text' AND text_end_offered AND text_progress IS NOT NULL
        AND (text_progress->>'end_accepted')::boolean IS TRUE
        AND (text_progress->>'written_utf8_bytes')::bigint IS NOT NULL
        AND (text_progress->>'written_utf8_bytes')::bigint=text_offered_utf8_bytes)
);
ALTER TABLE deliveries ADD CONSTRAINT deliveries_completed_evidence CHECK (status<>'completed' OR
    (media_kind='audio' AND end_accepted AND presented_frames IS NOT NULL AND presented_frames=offered_frames)
);
CREATE UNIQUE INDEX deliveries_text_identity ON deliveries((text_identity->>'text_id')) WHERE media_kind='text' AND text_identity IS NOT NULL;

CREATE TABLE delivery_text_chunks (
    delivery_id UUID NOT NULL REFERENCES deliveries(delivery_id),
    sequence BIGINT NOT NULL CHECK (sequence BETWEEN 0 AND 1023),
    utf8_offset BIGINT NOT NULL CHECK (utf8_offset BETWEEN 0 AND 262144),
    text TEXT NOT NULL CHECK (octet_length(text) BETWEEN 1 AND 8192),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(delivery_id,sequence)
);
