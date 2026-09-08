-- Privacy causality is a logical version captured at trusted source admission.
-- Server wall-clock timestamps and PostgreSQL timestamps may be in different
-- clock domains. NULL means unstamped, never the current privacy version.
ALTER TABLE chat_messages
    ADD COLUMN source_privacy_epoch BIGINT
    CHECK (source_privacy_epoch >= 0);

ALTER TABLE execution_effects
    ADD COLUMN source_privacy_epoch BIGINT
    CHECK (source_privacy_epoch >= 0);

CREATE INDEX chat_messages_source_epoch_idx
    ON chat_messages(user_id, source_privacy_epoch)
    WHERE role = 'user' AND source_privacy_epoch IS NOT NULL;
