//! Embodied expression runtime.
//!
//! The runtime interprets raw input through appraisal engines, keeps
//! appraisal/relationship state, regulates it into an expressed stance, and
//! exposes extension hooks for Rust components/plugins to contribute appraisal
//! engines, reducers, planners, and safety policies.

pub mod abi;
pub mod affect;
pub mod appraisal;
pub mod eval;
pub mod extension;
pub mod orchestrator;
pub mod regulation;
pub mod runtime;
pub mod trace;
pub mod types;

pub use abi::{
    EXPRESSION_PLUGIN_ABI_VERSION, ExpressionPluginCapabilities, ExpressionPluginManifest,
    ExpressionPluginRequest, ExpressionPluginResponse, ExternalAffectReducerContext,
    ExternalAppraisalContext, ExternalAppraisalEventDraft, ExternalExpressionPlugin,
};
pub use affect::{
    AffectAssistantResponse, AffectDeltas, AffectObservation, AffectStore, AffectUserInput,
    AppraisalDecay, AppraisalDecayKind, AppraisalEvent, AppraisalEvidence, AppraisalScope,
    AppraisalSource, AppraisalTarget,
};
pub use appraisal::{
    AppraisalContext, AppraisalEnginePlugin, RuleBasedAppraisalEngine,
    TextClassifierAppraisalEngine, classifier_user_appraisals, rule_based_user_appraisals,
};
pub use eval::{
    AppraisalRegressionCase, PlannerRegressionCase, appraisal_regression_cases,
    planner_regression_cases,
};
pub use extension::{
    AffectReducerContext, AffectReducerPlugin, ExpressionExtensionRegistry,
    ExpressionExtensionSummary, ExpressionPlannerPlugin, ExpressionPlanningContext,
    ExpressionSafetyPolicyPlugin, safety_modification,
};
pub use runtime::ExpressionRuntime;
pub use trace::{
    ExpressionTrace, ExpressionTraceEvent, ExpressionTraceEventSummary, ExpressionTraceIntent,
    ExpressionTraceObservation, expression_traces_from_events,
};
pub use types::{
    AffectAxes, AffectSnapshot, ExpressionActionCapability, ExpressionActionReceipt,
    ExpressionActionStatus, ExpressionAdapterState, ExpressionCapabilitySnapshot,
    ExpressionConstraints, ExpressionConversationContext, ExpressionDeviceContext,
    ExpressionExecutionResult, ExpressionInput, ExpressionIntent, ExpressionPlan,
    ExpressionPriority, ExpressionSafetyClass, ExpressionSafetyContext,
    ExpressionSafetyModification, ExpressionStyle, ExpressionTimelineAction, ExpressionUserState,
    ExpressiveStance, FallbackPolicy, ProsodyControls, RelationshipClearResult,
    RelationshipPrivacyControls, RelationshipProfile, RelationshipPruneResult, ResourceState,
    SkippedExpressionAction, SocialState, SpeechExpression,
};
