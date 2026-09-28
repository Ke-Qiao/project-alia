-- Live invitations never promote a shadow receipt. Charges and opaque identity
-- survive source/authority erasure; there is no replayable output outbox.
ALTER TABLE context_leases DROP CONSTRAINT context_leases_purpose_check;
ALTER TABLE context_leases ADD CONSTRAINT context_leases_purpose_check
    CHECK (purpose IN ('generation','source_notice','memory_capture','memory_maintenance','appraisal_rebuild','companion_invitation'));
ALTER TABLE companion_policies ADD COLUMN enabled_since TIMESTAMPTZ;
CREATE FUNCTION companion_policy_activation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.policy->>'mode'='enabled' THEN
        IF TG_OP='INSERT' OR NEW.revision IS DISTINCT FROM OLD.revision THEN NEW.enabled_since:=clock_timestamp(); END IF;
    ELSE NEW.enabled_since:=NULL;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER companion_policy_activation BEFORE INSERT OR UPDATE ON companion_policies
    FOR EACH ROW EXECUTE FUNCTION companion_policy_activation();

CREATE TABLE companion_source_claims (
    owner_id UUID NOT NULL REFERENCES users(id), source_event_id UUID NOT NULL,
    kind TEXT NOT NULL CHECK(kind='follow_up_invitation'), generator_version TEXT NOT NULL,
    origin_mode TEXT NOT NULL CHECK(origin_mode IN ('shadow','enabled')),
    policy_revision BIGINT NOT NULL CHECK(policy_revision>0),
    PRIMARY KEY(owner_id,source_event_id,kind,generator_version)
);
INSERT INTO companion_source_claims(owner_id,source_event_id,kind,generator_version,origin_mode,policy_revision)
    SELECT DISTINCT ON(s.owner_id,s.source_event_id) s.owner_id,s.source_event_id,'follow_up_invitation','alia.companion.invitation.v1','shadow',o.policy_revision
    FROM companion_processed_sources s JOIN companion_operations o USING(operation_id)
    ORDER BY s.owner_id,s.source_event_id,o.policy_revision;
CREATE FUNCTION companion_identity_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'Companion identity/charge cannot be removed or changed' USING ERRCODE='23514'; END;
$$;
CREATE TRIGGER companion_source_claim_immutable BEFORE UPDATE OR DELETE ON companion_source_claims
    FOR EACH ROW EXECUTE FUNCTION companion_identity_immutable();

CREATE TABLE companion_live_pending (
    candidate_id UUID PRIMARY KEY, owner_id UUID NOT NULL REFERENCES users(id), source_event_id UUID NOT NULL,
    kind TEXT NOT NULL CHECK(kind='follow_up_invitation'), generator_version TEXT NOT NULL,
    server_instance_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL, expires_at TIMESTAMPTZ NOT NULL CHECK(expires_at>created_at),
    state TEXT NOT NULL CHECK(state IN ('pending','admitted','retired')),
    retirement_reason TEXT CHECK(retirement_reason IN ('expired','source_revoked','policy_changed','instance_ended','attempt_failed','shutdown')),
    private_candidate JSONB CHECK(octet_length(private_candidate::text)<=32768),
    candidate_digest TEXT CHECK(candidate_digest~'^[0-9a-f]{64}$'),
    appraisal_id UUID,
    UNIQUE(owner_id,source_event_id,kind,generator_version),
    FOREIGN KEY(owner_id,source_event_id,kind,generator_version) REFERENCES companion_source_claims(owner_id,source_event_id,kind,generator_version),
    CHECK((state<>'retired')=(private_candidate IS NOT NULL AND candidate_digest IS NOT NULL AND appraisal_id IS NOT NULL)),
    CHECK(state<>'retired' OR(private_candidate IS NULL AND candidate_digest IS NULL AND appraisal_id IS NULL AND retirement_reason IS NOT NULL))
);
CREATE INDEX companion_live_pending_scan ON companion_live_pending(server_instance_id,created_at,candidate_id) WHERE state='pending';
CREATE TABLE companion_admissions (
    admission_id UUID PRIMARY KEY, candidate_id UUID NOT NULL UNIQUE REFERENCES companion_live_pending(candidate_id),
    owner_id UUID NOT NULL REFERENCES users(id), initiative_request_id UUID NOT NULL,
    output_key TEXT NOT NULL CHECK(output_key='companion_invitation'),
    context_id UUID NOT NULL REFERENCES context_leases(context_id) DEFERRABLE INITIALLY DEFERRED,
    conversation_id UUID NOT NULL,
    server_instance_id UUID NOT NULL, opportunity_id UUID NOT NULL,
    charged_at TIMESTAMPTZ NOT NULL, budget_date DATE NOT NULL,
    budget_revision BIGINT NOT NULL CHECK(budget_revision>0),
    admit_before TIMESTAMPTZ NOT NULL CHECK(admit_before>charged_at),
    valid_until TIMESTAMPTZ NOT NULL CHECK(valid_until>=admit_before),
    state TEXT NOT NULL CHECK(state IN ('active','retired')),
    retirement_reason TEXT CHECK(retirement_reason IN ('expired','source_revoked','policy_changed','instance_ended','attempt_failed','shutdown')),
    private_grant JSONB CHECK(octet_length(private_grant::text)<=65536),
    grant_digest TEXT CHECK(grant_digest~'^[0-9a-f]{64}$'),
    intent_digest TEXT CHECK(intent_digest~'^[0-9a-f]{64}$'),
    UNIQUE(owner_id,initiative_request_id,output_key), UNIQUE(owner_id,budget_revision),
    UNIQUE(server_instance_id,opportunity_id),
    CHECK(budget_date=(charged_at AT TIME ZONE 'UTC')::date),
    CHECK((state='active')=(private_grant IS NOT NULL AND grant_digest IS NOT NULL AND intent_digest IS NOT NULL)),
    CHECK(state<>'retired' OR(private_grant IS NULL AND grant_digest IS NULL AND intent_digest IS NULL AND retirement_reason IS NOT NULL))
);
CREATE INDEX companion_admission_day ON companion_admissions(owner_id,budget_date);
CREATE INDEX companion_admission_recent ON companion_admissions(owner_id,charged_at DESC);
CREATE INDEX companion_admission_expiry ON companion_admissions(valid_until,admission_id) WHERE state='active';
-- Charge time alone is insufficient: source/Context/Response writes may have
-- consumed the original opportunity. Enforce its strict boundary at commit.
CREATE FUNCTION companion_admission_commit_deadline() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF clock_timestamp() >= NEW.admit_before THEN
        RAISE EXCEPTION 'Original Companion admission opportunity expired' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE CONSTRAINT TRIGGER companion_admission_commit_deadline
    AFTER INSERT ON companion_admissions DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION companion_admission_commit_deadline();
CREATE TABLE companion_live_sources (
    candidate_id UUID NOT NULL REFERENCES companion_live_pending(candidate_id),
    kind TEXT NOT NULL CHECK(kind IN ('appraisal','affect','message','memory','graph','cache','execution','response')),
    source_id UUID NOT NULL, PRIMARY KEY(candidate_id,kind,source_id)
);
CREATE INDEX companion_live_source_erasure ON companion_live_sources(kind,source_id);
CREATE FUNCTION companion_live_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN RAISE EXCEPTION 'Companion charges and identities cannot be deleted' USING ERRCODE='23514'; END IF;
    IF TG_TABLE_NAME='companion_live_pending' THEN
        IF (to_jsonb(NEW)-'state'-'retirement_reason'-'private_candidate'-'candidate_digest'-'appraisal_id') IS DISTINCT FROM
           (to_jsonb(OLD)-'state'-'retirement_reason'-'private_candidate'-'candidate_digest'-'appraisal_id')
           OR (NEW IS DISTINCT FROM OLD AND NOT (
                (OLD.state='pending' AND NEW.state='admitted' AND NEW.private_candidate IS NOT DISTINCT FROM OLD.private_candidate
                 AND NEW.candidate_digest IS NOT DISTINCT FROM OLD.candidate_digest AND NEW.appraisal_id IS NOT DISTINCT FROM OLD.appraisal_id AND NEW.retirement_reason IS NULL)
                OR (OLD.state<>'retired' AND NEW.state='retired' AND NEW.private_candidate IS NULL AND NEW.candidate_digest IS NULL AND NEW.appraisal_id IS NULL)))
        THEN RAISE EXCEPTION 'Companion pending intent is immutable' USING ERRCODE='23514'; END IF;
    ELSE
        IF (to_jsonb(NEW)-'state'-'retirement_reason'-'private_grant'-'grant_digest'-'intent_digest') IS DISTINCT FROM
           (to_jsonb(OLD)-'state'-'retirement_reason'-'private_grant'-'grant_digest'-'intent_digest')
           OR (NEW IS DISTINCT FROM OLD AND NOT (OLD.state='active' AND NEW.state='retired' AND NEW.private_grant IS NULL AND NEW.grant_digest IS NULL AND NEW.intent_digest IS NULL))
        THEN RAISE EXCEPTION 'Companion grant/charge is immutable' USING ERRCODE='23514'; END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER companion_pending_immutable BEFORE UPDATE OR DELETE ON companion_live_pending FOR EACH ROW EXECUTE FUNCTION companion_live_immutable();
CREATE TRIGGER companion_admission_immutable BEFORE UPDATE OR DELETE ON companion_admissions FOR EACH ROW EXECUTE FUNCTION companion_live_immutable();

CREATE FUNCTION retire_companion_live(p_candidates UUID[], p_reason TEXT) RETURNS void
LANGUAGE plpgsql SET search_path FROM CURRENT AS $$
BEGIN
    -- Candidate -> admission -> Response; never reach back into source rows.
    UPDATE companion_live_pending SET state='retired',retirement_reason=p_reason,private_candidate=NULL,candidate_digest=NULL,appraisal_id=NULL
        WHERE candidate_id=ANY(p_candidates) AND state<>'retired';
    UPDATE companion_admissions SET state='retired',retirement_reason=p_reason,private_grant=NULL,grant_digest=NULL,intent_digest=NULL
        WHERE candidate_id=ANY(p_candidates) AND state='active';
    UPDATE delivery_responses r SET cancel_requested=TRUE FROM companion_admissions a
        WHERE a.candidate_id=ANY(p_candidates) AND r.user_id=a.owner_id AND r.request_id=a.initiative_request_id AND r.output_key=a.output_key AND NOT r.cancel_requested;
    DELETE FROM companion_live_sources WHERE candidate_id=ANY(p_candidates);
END;
$$;
CREATE FUNCTION retire_companion_live_authority() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $$
DECLARE owner UUID; changed BOOLEAN; reason TEXT;
BEGIN
    reason:='policy_changed';
    IF TG_TABLE_NAME='memory_privacy_state' THEN owner:=NEW.user_id; changed:=NEW.privacy_epoch IS DISTINCT FROM OLD.privacy_epoch; reason:='source_revoked';
    ELSIF TG_TABLE_NAME='relationship_authority' THEN owner:=NEW.owner_id; changed:=NEW.relationship_epoch IS DISTINCT FROM OLD.relationship_epoch; reason:='source_revoked';
    ELSIF TG_TABLE_NAME='companion_policies' THEN owner:=NEW.owner_id; changed:=NEW.revision IS DISTINCT FROM OLD.revision;
    ELSE changed:=NEW.version IS DISTINCT FROM OLD.version; END IF;
    IF changed THEN PERFORM retire_companion_live(ARRAY(SELECT candidate_id FROM companion_live_pending WHERE state<>'retired' AND (owner IS NULL OR owner_id=owner)),reason); END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER privacy_retires_companion_live AFTER UPDATE OF privacy_epoch ON memory_privacy_state FOR EACH ROW EXECUTE FUNCTION retire_companion_live_authority();
CREATE TRIGGER relationship_retires_companion_live AFTER UPDATE OF relationship_epoch ON relationship_authority FOR EACH ROW EXECUTE FUNCTION retire_companion_live_authority();
CREATE TRIGGER policy_retires_companion_live AFTER UPDATE OF revision ON companion_policies FOR EACH ROW EXECUTE FUNCTION retire_companion_live_authority();
CREATE TRIGGER context_policy_retires_companion_live AFTER UPDATE OF version ON context_policy FOR EACH ROW EXECUTE FUNCTION retire_companion_live_authority();
CREATE FUNCTION retire_companion_live_source() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $$
DECLARE field TEXT; changed BOOLEAN:=FALSE;
BEGIN
    IF TG_OP='UPDATE' THEN
        IF NEW IS NOT DISTINCT FROM OLD THEN RETURN NEW; END IF;
        IF TG_NARGS>2 THEN
            FOREACH field IN ARRAY string_to_array(TG_ARGV[2],',') LOOP
                IF (to_jsonb(NEW)->field) IS DISTINCT FROM (to_jsonb(OLD)->field) THEN changed:=TRUE; EXIT; END IF;
            END LOOP;
            IF NOT changed THEN RETURN NEW; END IF;
        END IF;
    END IF;
    PERFORM retire_companion_live(ARRAY(SELECT candidate_id FROM companion_live_sources WHERE kind=TG_ARGV[0] AND source_id=(to_jsonb(OLD)->>TG_ARGV[1])::uuid),'source_revoked');
    IF TG_OP='DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END;
$$;
CREATE TRIGGER appraisal_retires_companion_live AFTER UPDATE OR DELETE ON appraisal_records FOR EACH ROW EXECUTE FUNCTION retire_companion_live_source('appraisal','appraisal_id');
CREATE TRIGGER affect_retires_companion_live AFTER DELETE ON affect_projections FOR EACH ROW EXECUTE FUNCTION retire_companion_live_source('affect','projection_id');
CREATE TRIGGER message_retires_companion_live AFTER UPDATE OF content,user_id,session_id,role,source_received_at,source_privacy_epoch,source_relationship_epoch OR DELETE ON chat_messages FOR EACH ROW EXECUTE FUNCTION retire_companion_live_source('message','id');
CREATE TRIGGER memory_retires_companion_live AFTER UPDATE OF revision,status,visibility,sensitive,expires_at OR DELETE ON memory_items FOR EACH ROW EXECUTE FUNCTION retire_companion_live_source('memory','id');
CREATE TRIGGER graph_retires_companion_live AFTER UPDATE OR DELETE ON memory_edges FOR EACH ROW EXECUTE FUNCTION retire_companion_live_source('graph','id');
CREATE TRIGGER cache_retires_companion_live AFTER UPDATE OR DELETE ON llm_cache FOR EACH ROW EXECUTE FUNCTION retire_companion_live_source('cache','id','id,user_id,context_id,revision,scope,query_hash,query_text,response_text,content_digest,status,expires_at');
CREATE TRIGGER execution_retires_companion_live AFTER UPDATE OR DELETE ON execution_effects FOR EACH ROW EXECUTE FUNCTION retire_companion_live_source('execution','effect_id','effect_id,user_id,request_id,model_call_id,tool_name,status,intent_digest,completion,context_id,source_event_id,source_privacy_epoch,source_received_at');
CREATE TRIGGER response_retires_companion_live AFTER UPDATE OR DELETE ON delivery_responses FOR EACH ROW EXECUTE FUNCTION retire_companion_live_source('response','response_id','response_id,user_id,request_id,context_id,conversation_id,assistant_message_id,status,final_content,source_event_id,source_privacy_epoch,source_received_at,channel_id');
CREATE FUNCTION retire_companion_live_graph_node() RETURNS trigger LANGUAGE plpgsql SET search_path FROM CURRENT AS $$
BEGIN
    IF TG_OP='UPDATE' AND (NEW.id,NEW.user_id,NEW.tenant_id,NEW.label,NEW.node_type,NEW.properties) IS NOT DISTINCT FROM (OLD.id,OLD.user_id,OLD.tenant_id,OLD.label,OLD.node_type,OLD.properties) THEN RETURN NEW; END IF;
    PERFORM retire_companion_live(ARRAY(SELECT s.candidate_id FROM companion_live_sources s JOIN memory_edges e ON e.id=s.source_id WHERE s.kind='graph' AND (e.src_node_id=OLD.id OR e.dst_node_id=OLD.id)),'source_revoked');
    IF TG_OP='DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END;
$$;
CREATE TRIGGER graph_node_retires_companion_live BEFORE UPDATE OR DELETE ON memory_nodes FOR EACH ROW EXECUTE FUNCTION retire_companion_live_graph_node();
