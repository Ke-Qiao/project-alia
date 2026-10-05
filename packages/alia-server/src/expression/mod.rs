//! Embodied expression runtime.
//!
//! Interpretation consumes canonical Context inputs. A supervised writer
//! commits versioned appraisal records before publishing receipts; projections
//! replay only validated records. The runtime holds no private relationship
//! cache. Source-free planning is a preview and grants no output authority.

pub mod abi;
pub mod affect;
pub mod appraisal;
pub mod eval;
pub mod extension;
pub mod orchestrator;
mod provenance;
pub mod regulation;
pub mod runtime;
pub mod trace;
pub mod types;

pub use abi::{
    EXPRESSION_PLUGIN_ABI_VERSION, ExpressionPluginCapabilities, ExpressionPluginManifest,
    ExpressionPluginRequest, ExpressionPluginResponse, ExternalAppraisalContext,
    ExternalAppraisalEventDraft, ExternalExpressionPlugin,
};
pub use affect::{
    AffectAssistantResponse, AffectDeltas, AppraisalDecay, AppraisalDecayKind, AppraisalEvent,
    AppraisalEvidence, AppraisalScope, AppraisalSource, AppraisalTarget,
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
    ExpressionExtensionRegistry, ExpressionExtensionSummary, ExpressionPlannerPlugin,
    ExpressionPlanningContext, ExpressionSafetyPolicyPlugin, safety_modification,
};
pub use runtime::ExpressionRuntime;
pub use trace::{
    ExpressionTrace, ExpressionTraceEvent, ExpressionTraceEventSummary,
    expression_traces_from_events,
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
