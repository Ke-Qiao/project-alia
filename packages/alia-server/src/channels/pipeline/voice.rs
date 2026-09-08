use super::*;

pub(super) enum PipelineInput {
    Legacy(ChannelEvent),
    Recognized(RecognizedVoiceInput),
}

pub(super) async fn process_recognized_voice(
    input: RecognizedVoiceInput,
    modules: &ModuleRegistry,
    channels: &ChannelRegistry,
    event_bus: &EventBus,
    voice_turns: &VoiceTurnRegistry,
    debug_output: bool,
    policy: &PipelinePolicies,
) -> anyhow::Result<()> {
    if input.cancel.is_cancelled() || input.transcript.trim().is_empty() {
        return Ok(());
    }
    let (channel_id, channel) = resolve_event_channel(channels, &input.channel_id).await?;
    if input.cancel.is_cancelled() {
        return Ok(());
    }
    let target = ChannelTarget {
        voice: Some(VoiceSessionScope {
            session_id: input.session_id,
            device_boot_id: input.device_boot_id,
            generation_epoch: input.generation_epoch,
        }),
        user_id: input.user_id.clone(),
    };
    let mut interrupt_config = channel.interrupt_config();
    // Cancellation is owned by the live session, even if this adapter does not
    // support the legacy wakeword interrupt policy.
    interrupt_config.enabled = true;
    let mut turn = voice_turns
        .register(
            channel_id.clone(),
            input.user_id.clone(),
            Uuid::new_v4(),
            interrupt_config,
            input.cancel,
        )
        .await;
    turn.voice_scope = Some(VoiceSessionScope {
        session_id: input.session_id,
        device_boot_id: input.device_boot_id,
        generation_epoch: input.generation_epoch,
    });
    turn.output_channel = Some(channel.clone());
    turn.received_at = input.source_received_at;
    turn.source_privacy_epoch = Some(input.source_privacy_epoch);
    tracing::debug!(session_id = %input.session_id, device_boot_id = %input.device_boot_id,
        generation_epoch = input.generation_epoch, "processing recognized voice input");
    let result = process_voice_response(
        &input.transcript,
        &input.user_id,
        modules,
        channel.as_ref(),
        &target,
        &channel_id,
        event_bus,
        debug_output,
        &policy.conversation,
        &policy.tools,
        &policy.cache,
        &policy.cache_policy,
        &policy.memory,
        &turn,
        None,
    )
    .await;
    voice_turns.finish(&turn).await;
    result
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn process_voice_response(
    text: &str,
    user_id: &str,
    modules: &ModuleRegistry,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    channel_id: &str,
    event_bus: &EventBus,
    debug_output: bool,
    conversation_config: &ConversationSection,
    tools_config: &ToolsSection,
    cache_config: &CacheSection,
    cache_policy: &CachePolicyResourceSection,
    memory_config: &MemorySection,
    turn: &VoiceTurn,
    correction_trace: Option<TranscriptCorrectionResult>,
) -> anyhow::Result<()> {
    let result = match process_text(
        text,
        user_id,
        modules,
        channel,
        target,
        channel_id,
        event_bus,
        debug_output,
        channel.capabilities().voice_output && channel.capabilities().audio_streaming,
        true,
        conversation_config,
        tools_config,
        cache_config,
        cache_policy,
        memory_config,
        Some(turn),
        true,
        correction_trace,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            notify_pipeline_error(
                modules,
                channel,
                target,
                channel_id,
                user_id,
                PipelineErrorStage::Llm,
                Some(text),
                Some(&error),
                debug_output,
                true,
                Some(turn),
            )
            .await;
            return Ok(());
        }
    };

    if result.interrupted || turn_is_interrupted(Some(turn)) {
        return Ok(());
    }

    // Text delivery: only if not already streamed
    if !result.streamed {
        debug_component_text(
            debug_output,
            "channel",
            "send_message",
            &result.text,
            Some((channel_id, user_id)),
        );
        if let Err(error) = cancellable_stage(
            Some(turn),
            "text_delivery",
            channel.send_message(target, &result.text),
        )
        .await
        {
            log_channel_delivery_failure(channel, error).await;
            return Ok(());
        }
    }

    if let Some(plan) = &result.expression_plan {
        execute_expression_and_trace(
            modules.expression.orchestrator(),
            channel,
            target,
            plan,
            Some(ExpressionExecutionPhase::PreSpeech),
            debug_output,
            channel_id,
            user_id,
            Some(turn),
        )
        .await;
    }

    if channel.capabilities().voice_output && !result.audio_streamed {
        let tts_delivery = deliver_tts_audio(
            modules,
            channel,
            target,
            channel_id,
            user_id,
            &result.text,
            result
                .expression_plan
                .as_ref()
                .and_then(|plan| plan.speech.as_ref()),
            result.expression_plan.as_ref(),
            result.expression_input.as_ref(),
            result.tts_trace.clone(),
            debug_output,
            Some(turn),
        )
        .await;
        if let TtsDeliveryOutcome::Failed(stage) = tts_delivery
            && !turn_is_interrupted(Some(turn))
        {
            notify_pipeline_error(
                modules,
                channel,
                target,
                channel_id,
                user_id,
                stage,
                Some(text),
                None,
                debug_output,
                true,
                Some(turn),
            )
            .await;
        }
    }

    let presented_finished = if let Some(response_id) = turn.response_id.get() {
        match modules
            .delivery
            .list_deliveries(Uuid::parse_str(user_id)?, *response_id)
            .await
        {
            Ok(deliveries) => {
                !deliveries.is_empty()
                    && deliveries.iter().all(|delivery| {
                        delivery.status
                            == crate::modules::delivery::types::DeliveryStatus::Completed
                    })
            }
            Err(_) => false,
        }
    } else {
        false
    };
    if !presented_finished && result.expression_plan.is_some() {
        if let Some(trace) = &result.tts_trace {
            trace
                .record(
                    "post_speech_skipped",
                    "delivery",
                    Some("Device presentation completion was not proven".into()),
                    serde_json::json!({"reason":"unsupported_evidence"}),
                )
                .await;
        }
    }
    if let Some(plan) = &result.expression_plan
        && presented_finished
    {
        execute_expression_and_trace(
            modules.expression.orchestrator(),
            channel,
            target,
            plan,
            Some(ExpressionExecutionPhase::PostSpeech),
            debug_output,
            channel_id,
            user_id,
            Some(turn),
        )
        .await;
    }

    if turn_is_interrupted(Some(turn)) {
        turn.mark_interrupted_stage("post_speech_delivery");
        if let Some(pending) = result.pending_persistence.as_ref() {
            persist_interrupted_user_context(modules, pending, turn, debug_output).await;
        }
        return Ok(());
    }

    if let Some(pending) = result.pending_persistence {
        persist_text_interaction(modules, pending, memory_config, event_bus, None).await;
    }

    Ok(())
}
