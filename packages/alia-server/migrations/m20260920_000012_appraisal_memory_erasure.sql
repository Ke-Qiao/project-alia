-- A revoked Appraisal cannot leave canonical Memory copies behind. Both
-- explicit revocation paths and the privacy trigger use this single eraser.
-- Callers own policy/account/privacy/relationship locks before evidence locks.
CREATE FUNCTION erase_memory_closure(p_owner UUID, p_tenant UUID, p_roots UUID[])
RETURNS BIGINT LANGUAGE plpgsql SET search_path FROM CURRENT AS $$
DECLARE
    erased_ids UUID[];
    erased_count BIGINT;
BEGIN
    WITH RECURSIVE erased(id) AS (
        SELECT m.id FROM memory_items m
        WHERE m.user_id=p_owner AND m.tenant_id=p_tenant AND m.id=ANY(p_roots)
        UNION
        SELECT m.id FROM memory_sources s
        JOIN erased e ON e.id=s.source_id
        JOIN memory_items m ON m.id=s.memory_id
        WHERE m.user_id=p_owner AND m.tenant_id=p_tenant
    )
    SELECT array_agg(id ORDER BY id) INTO erased_ids FROM erased;
    IF erased_ids IS NULL THEN RETURN 0; END IF;

    PERFORM id FROM memory_items
        WHERE user_id=p_owner AND tenant_id=p_tenant AND id=ANY(erased_ids)
        ORDER BY id FOR UPDATE;
    DELETE FROM memory_embeddings e USING memory_items m
        WHERE e.memory_id=m.id AND m.user_id=p_owner AND m.tenant_id=p_tenant
          AND m.id=ANY(erased_ids);
    DELETE FROM memory_edges e
        WHERE e.user_id=p_owner AND e.tenant_id=p_tenant
          AND (e.evidence_memory_id=ANY(erased_ids)
            OR e.src_node_id IN (
                SELECT n.id FROM memory_nodes n
                WHERE n.user_id=p_owner AND n.tenant_id=p_tenant
                  AND (n.properties->>'memory_id' IN (SELECT v::text FROM unnest(erased_ids) v)
                    OR n.node_key IN (SELECT 'memory:'||v::text FROM unnest(erased_ids) v)))
            OR e.dst_node_id IN (
                SELECT n.id FROM memory_nodes n
                WHERE n.user_id=p_owner AND n.tenant_id=p_tenant
                  AND (n.properties->>'memory_id' IN (SELECT v::text FROM unnest(erased_ids) v)
                    OR n.node_key IN (SELECT 'memory:'||v::text FROM unnest(erased_ids) v))));
    DELETE FROM memory_nodes n
        WHERE n.user_id=p_owner AND n.tenant_id=p_tenant
          AND (n.properties->>'memory_id' IN (SELECT v::text FROM unnest(erased_ids) v)
            OR n.node_key IN (SELECT 'memory:'||v::text FROM unnest(erased_ids) v));
    DELETE FROM memory_sources s USING memory_items m
        WHERE s.memory_id=m.id AND m.user_id=p_owner AND m.tenant_id=p_tenant
          AND (s.memory_id=ANY(erased_ids) OR s.source_id=ANY(erased_ids));
    DELETE FROM memory_appraisal_sources a USING memory_items m
        WHERE a.memory_id=m.id AND a.owner_id=p_owner
          AND m.user_id=p_owner AND m.tenant_id=p_tenant AND m.id=ANY(erased_ids);
    DELETE FROM memory_message_sources s USING memory_items m
        WHERE s.memory_id=m.id AND s.owner_id=p_owner
          AND m.user_id=p_owner AND m.tenant_id=p_tenant AND m.id=ANY(erased_ids);
    UPDATE memory_items
        SET status='deleted',revision=revision+1,content='',summary=NULL,
            evidence='{}',metadata='{}',topic_tags='{}',affect_tags='{}',
            source_event_ids='{}',superseded_by=NULL,
            expires_at=clock_timestamp(),updated_at=clock_timestamp()
        WHERE user_id=p_owner AND tenant_id=p_tenant AND id=ANY(erased_ids)
          AND status<>'deleted';
    GET DIAGNOSTICS erased_count = ROW_COUNT;
    DELETE FROM memory_nodes n
        WHERE n.user_id=p_owner AND n.tenant_id=p_tenant
          AND NOT EXISTS (
              SELECT 1 FROM memory_edges e WHERE e.src_node_id=n.id OR e.dst_node_id=n.id);
    RETURN erased_count;
END;
$$;

CREATE OR REPLACE FUNCTION erase_private_appraisal_derivatives()
RETURNS trigger LANGUAGE plpgsql SET search_path FROM CURRENT AS $$
BEGIN
    IF NEW.privacy_epoch IS DISTINCT FROM OLD.privacy_epoch THEN
        PERFORM 1 FROM relationship_authority WHERE owner_id=NEW.user_id FOR UPDATE;
        PERFORM erase_memory_closure(NEW.user_id,'00000000-0000-0000-0000-000000000000',
            ARRAY(SELECT a.memory_id FROM memory_appraisal_sources a
                JOIN memory_items m ON m.id=a.memory_id
                WHERE a.owner_id=NEW.user_id AND m.user_id=NEW.user_id
                  AND m.tenant_id='00000000-0000-0000-0000-000000000000'));
        DELETE FROM affect_projections WHERE owner_id=NEW.user_id;
        DELETE FROM appraisal_records WHERE owner_id=NEW.user_id;
        DELETE FROM conversation_events e USING chat_sessions s
            WHERE e.session_id=s.id AND s.creator_id=NEW.user_id::text
              AND e.actor IN ('affect','expression');
    END IF;
    RETURN NEW;
END;
$$;

-- Repair copies left by an earlier privacy transition. Never reconstruct an
-- old identity from current text or erase valid outputs of the current epoch.
-- Text comparison also rejects absent/malformed old JSON epoch identities.
DO $$
DECLARE
    owner UUID;
    stale_ids UUID[];
BEGIN
    PERFORM singleton FROM context_policy WHERE singleton FOR SHARE;
    FOR owner IN
        SELECT DISTINCT a.owner_id FROM memory_appraisal_sources a
        JOIN memory_items m ON m.id=a.memory_id AND m.user_id=a.owner_id
        LEFT JOIN memory_privacy_state p ON p.user_id=a.owner_id
        WHERE m.tenant_id='00000000-0000-0000-0000-000000000000'
          AND (p.user_id IS NULL
            OR (a.source->>'source_privacy_epoch') IS DISTINCT FROM p.privacy_epoch::text)
        ORDER BY a.owner_id
    LOOP
        PERFORM id FROM users WHERE id=owner FOR SHARE;
        PERFORM user_id FROM memory_privacy_state WHERE user_id=owner FOR UPDATE;
        PERFORM owner_id FROM relationship_authority WHERE owner_id=owner FOR UPDATE;
        SELECT array_agg(a.memory_id ORDER BY a.memory_id) INTO stale_ids
            FROM memory_appraisal_sources a
            JOIN memory_items m ON m.id=a.memory_id AND m.user_id=a.owner_id
            LEFT JOIN memory_privacy_state p ON p.user_id=a.owner_id
            WHERE a.owner_id=owner AND m.tenant_id='00000000-0000-0000-0000-000000000000'
              AND (p.user_id IS NULL
                OR (a.source->>'source_privacy_epoch') IS DISTINCT FROM p.privacy_epoch::text);
        PERFORM erase_memory_closure(owner,'00000000-0000-0000-0000-000000000000',stale_ids);
    END LOOP;
END;
$$;
