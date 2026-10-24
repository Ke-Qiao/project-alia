-- Rename the short, bounded physical motion domain without rewriting signed
-- command/progress evidence or granting permission to replay historical work.
ALTER TABLE body_deliveries RENAME TO motion_deliveries;
ALTER TABLE motion_deliveries RENAME CONSTRAINT body_deliveries_pkey TO motion_deliveries_pkey;
ALTER TABLE motion_deliveries RENAME CONSTRAINT body_deliveries_response_id_fkey TO motion_deliveries_response_id_fkey;
ALTER TABLE motion_deliveries RENAME CONSTRAINT body_deliveries_output_key_check TO motion_deliveries_output_key_check;
ALTER TABLE motion_deliveries RENAME CONSTRAINT body_deliveries_dispatch_id_key TO motion_deliveries_dispatch_id_key;
ALTER TABLE motion_deliveries RENAME CONSTRAINT body_deliveries_identity_key TO motion_deliveries_identity_key;
ALTER TABLE motion_deliveries RENAME CONSTRAINT body_deliveries_status_check TO motion_deliveries_status_check;
ALTER TABLE motion_deliveries RENAME CONSTRAINT body_deliveries_response_id_output_key_key TO motion_deliveries_response_id_output_key_key;
ALTER TABLE motion_deliveries RENAME CONSTRAINT body_deliveries_channel_id_source_command_id_key TO motion_deliveries_channel_id_source_command_id_key;
ALTER TABLE motion_deliveries RENAME CONSTRAINT body_deliveries_check TO motion_deliveries_check;
ALTER INDEX body_deliveries_expiry RENAME TO motion_deliveries_expiry;

ALTER TABLE body_delivery_late_checkpoints RENAME TO motion_delivery_late_checkpoints;
ALTER TABLE motion_delivery_late_checkpoints RENAME CONSTRAINT body_delivery_late_checkpoints_pkey TO motion_delivery_late_checkpoints_pkey;
ALTER TABLE motion_delivery_late_checkpoints RENAME CONSTRAINT body_delivery_late_checkpoints_delivery_id_fkey TO motion_delivery_late_checkpoints_delivery_id_fkey;
ALTER TABLE motion_delivery_late_checkpoints RENAME CONSTRAINT body_delivery_late_checkpoints_receipt_sequence_check TO motion_delivery_late_checkpoints_receipt_sequence_check;

-- Audit envelope vocabulary changes; the embedded command/progress facts do not.
UPDATE delivery_events
SET kind = CASE kind
    WHEN 'body_dispatch' THEN 'motion_dispatch'
    WHEN 'body_progress' THEN 'motion_progress'
    WHEN 'body_late_checkpoint' THEN 'motion_late_checkpoint'
    WHEN 'body_unknown' THEN 'motion_unknown'
END,
payload = CASE WHEN payload ? 'body_delivery_id'
    THEN (payload - 'body_delivery_id') || jsonb_build_object('motion_delivery_id', payload->'body_delivery_id')
    ELSE payload END
WHERE kind IN ('body_dispatch', 'body_progress', 'body_late_checkpoint', 'body_unknown');
