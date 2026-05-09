use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::entity::{persona, user_active_persona, user_persona_override};
use crate::error::{AppError, Result};
use crate::state::AppState;

// ========== Response types ==========

#[derive(Serialize)]
pub struct PersonaInfo {
    pub id: Uuid,
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub is_system: bool,
    pub greeting: Option<String>,
    pub personality_traits: serde_json::Value,
    pub voice_config: serde_json::Value,
    pub behavior_guidelines: Option<String>,
    pub sort_order: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<persona::Model> for PersonaInfo {
    fn from(m: persona::Model) -> Self {
        Self {
            id: m.id,
            name: m.name,
            display_name: m.display_name,
            description: m.description,
            is_system: m.is_system,
            greeting: m.greeting,
            personality_traits: m.personality_traits,
            voice_config: m.voice_config,
            behavior_guidelines: m.behavior_guidelines,
            sort_order: m.sort_order,
            created_at: m.created_at,
            updated_at: m.updated_at,
        }
    }
}

#[derive(Serialize)]
pub struct EffectivePersona {
    pub persona_id: Uuid,
    pub name: String,
    pub display_name: String,
    pub greeting: Option<String>,
    pub personality_traits: serde_json::Value,
    pub voice_config: serde_json::Value,
    pub behavior_guidelines: Option<String>,
    pub has_overrides: bool,
}

// ========== Request types ==========

#[derive(Deserialize)]
pub struct CreatePersonaRequest {
    pub name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub system_prompt_template: String,
    #[serde(default = "default_traits")]
    pub personality_traits: serde_json::Value,
    #[serde(default = "default_voice")]
    pub voice_config: serde_json::Value,
    #[serde(default)]
    pub greeting: Option<String>,
    #[serde(default)]
    pub behavior_guidelines: Option<String>,
    #[serde(default)]
    pub sort_order: i32,
}

fn default_traits() -> serde_json::Value {
    serde_json::json!({})
}

fn default_voice() -> serde_json::Value {
    serde_json::json!({})
}

#[derive(Deserialize)]
pub struct UpdatePersonaRequest {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub system_prompt_template: Option<String>,
    #[serde(default)]
    pub personality_traits: Option<serde_json::Value>,
    #[serde(default)]
    pub voice_config: Option<serde_json::Value>,
    #[serde(default)]
    pub greeting: Option<String>,
    #[serde(default)]
    pub behavior_guidelines: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub sort_order: Option<i32>,
}

#[derive(Deserialize)]
pub struct UserOverrideRequest {
    #[serde(default)]
    pub persona_id: Option<Uuid>,
    #[serde(default)]
    pub greeting: Option<String>,
    #[serde(default)]
    pub behavior_guidelines: Option<String>,
    #[serde(default)]
    pub personality_traits: Option<serde_json::Value>,
    #[serde(default)]
    pub voice_config: Option<serde_json::Value>,
}

// ========== Handlers ==========

/// GET /personas — list all enabled personas
async fn list_personas(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<PersonaInfo>>> {
    let personas = persona::Entity::find()
        .filter(persona::Column::Enabled.eq(true))
        .order_by_asc(persona::Column::SortOrder)
        .all(&state.db)
        .await
        .map_err(AppError::Database)?;

    Ok(Json(personas.into_iter().map(PersonaInfo::from).collect()))
}

/// GET /personas/:id — persona detail
async fn get_persona(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<PersonaInfo>> {
    let p = persona::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(AppError::Database)?
        .ok_or_else(|| AppError::NotFound("Persona not found".into()))?;

    Ok(Json(PersonaInfo::from(p)))
}

/// POST /personas — create persona (admin only)
async fn create_persona(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<CreatePersonaRequest>,
) -> Result<Json<PersonaInfo>> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }

    if body.name.is_empty() || body.display_name.is_empty() {
        return Err(AppError::Validation("name and display_name are required".into()));
    }

    let now = chrono::Utc::now();
    let model = persona::ActiveModel {
        id: Set(Uuid::new_v4()),
        name: Set(body.name),
        display_name: Set(body.display_name),
        description: Set(body.description),
        system_prompt_template: Set(body.system_prompt_template),
        personality_traits: Set(body.personality_traits),
        voice_config: Set(body.voice_config),
        greeting: Set(body.greeting),
        behavior_guidelines: Set(body.behavior_guidelines),
        is_system: Set(false),
        enabled: Set(true),
        sort_order: Set(body.sort_order),
        created_at: Set(now),
        updated_at: Set(now),
    };

    let result = model.insert(&state.db).await.map_err(AppError::Database)?;
    Ok(Json(PersonaInfo::from(result)))
}

/// PUT /personas/:id — update persona (admin only)
async fn update_persona(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdatePersonaRequest>,
) -> Result<Json<PersonaInfo>> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }

    let existing = persona::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(AppError::Database)?
        .ok_or_else(|| AppError::NotFound("Persona not found".into()))?;

    let mut active: persona::ActiveModel = existing.into();
    if let Some(v) = body.display_name {
        active.display_name = Set(v);
    }
    if let Some(v) = body.description {
        active.description = Set(Some(v));
    }
    if let Some(v) = body.system_prompt_template {
        active.system_prompt_template = Set(v);
    }
    if let Some(v) = body.personality_traits {
        active.personality_traits = Set(v);
    }
    if let Some(v) = body.voice_config {
        active.voice_config = Set(v);
    }
    if let Some(v) = body.greeting {
        active.greeting = Set(Some(v));
    }
    if let Some(v) = body.behavior_guidelines {
        active.behavior_guidelines = Set(Some(v));
    }
    if let Some(v) = body.enabled {
        active.enabled = Set(v);
    }
    if let Some(v) = body.sort_order {
        active.sort_order = Set(v);
    }
    active.updated_at = Set(chrono::Utc::now());

    let result = active.update(&state.db).await.map_err(AppError::Database)?;
    Ok(Json(PersonaInfo::from(result)))
}

/// DELETE /personas/:id — delete persona (admin only, cannot delete system personas)
async fn delete_persona(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }

    let existing = persona::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(AppError::Database)?
        .ok_or_else(|| AppError::NotFound("Persona not found".into()))?;

    if existing.is_system {
        return Err(AppError::Validation("Cannot delete system personas".into()));
    }

    persona::Entity::delete_by_id(id)
        .exec(&state.db)
        .await
        .map_err(AppError::Database)?;

    Ok(Json(serde_json::json!({"success": true})))
}

/// GET /personas/me — get the user's effective persona (base + overrides merged)
async fn get_my_persona(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<EffectivePersona>> {
    // Find active persona
    let base = find_user_active_persona(&state, auth.id).await?;

    // Find user override
    let user_override = user_persona_override::Entity::find()
        .filter(user_persona_override::Column::UserId.eq(auth.id))
        .filter(user_persona_override::Column::PersonaId.eq(base.id))
        .one(&state.db)
        .await
        .map_err(AppError::Database)?;

    let has_overrides = user_override.is_some();

    let effective = EffectivePersona {
        persona_id: base.id,
        name: base.name,
        display_name: base.display_name.clone(),
        greeting: user_override
            .as_ref()
            .and_then(|o| o.greeting.clone())
            .or(base.greeting),
        personality_traits: user_override
            .as_ref()
            .and_then(|o| o.personality_traits.clone())
            .unwrap_or(base.personality_traits),
        voice_config: user_override
            .as_ref()
            .and_then(|o| o.voice_config.clone())
            .unwrap_or(base.voice_config),
        behavior_guidelines: user_override
            .as_ref()
            .and_then(|o| o.behavior_guidelines.clone())
            .or(base.behavior_guidelines),
        has_overrides,
    };

    Ok(Json(effective))
}

/// PATCH /personas/me — set active persona and/or apply user overrides
async fn update_my_persona(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<UserOverrideRequest>,
) -> Result<Json<EffectivePersona>> {
    let now = chrono::Utc::now();

    // If persona_id is specified, update the active persona selection
    if let Some(persona_id) = body.persona_id {
        // Verify persona exists and is enabled
        persona::Entity::find_by_id(persona_id)
            .filter(persona::Column::Enabled.eq(true))
            .one(&state.db)
            .await
            .map_err(AppError::Database)?
            .ok_or_else(|| AppError::NotFound("Persona not found or disabled".into()))?;

        // Upsert active persona
        let active = user_active_persona::ActiveModel {
            user_id: Set(auth.id),
            persona_id: Set(persona_id),
            updated_at: Set(now),
        };

        // Try insert, on conflict update
        let existing = user_active_persona::Entity::find_by_id(auth.id)
            .one(&state.db)
            .await
            .map_err(AppError::Database)?;

        if existing.is_some() {
            active.update(&state.db).await.map_err(AppError::Database)?;
        } else {
            active.insert(&state.db).await.map_err(AppError::Database)?;
        }
    }

    // Get the (possibly just updated) active persona
    let base = find_user_active_persona(&state, auth.id).await?;

    // Apply overrides if any override fields are present
    let has_override_fields = body.greeting.is_some()
        || body.behavior_guidelines.is_some()
        || body.personality_traits.is_some()
        || body.voice_config.is_some();

    if has_override_fields {
        let existing_override = user_persona_override::Entity::find()
            .filter(user_persona_override::Column::UserId.eq(auth.id))
            .filter(user_persona_override::Column::PersonaId.eq(base.id))
            .one(&state.db)
            .await
            .map_err(AppError::Database)?;

        if let Some(existing) = existing_override {
            let mut active: user_persona_override::ActiveModel = existing.into();
            if body.greeting.is_some() {
                active.greeting = Set(body.greeting);
            }
            if body.behavior_guidelines.is_some() {
                active.behavior_guidelines = Set(body.behavior_guidelines);
            }
            if body.personality_traits.is_some() {
                active.personality_traits = Set(body.personality_traits);
            }
            if body.voice_config.is_some() {
                active.voice_config = Set(body.voice_config);
            }
            active.updated_at = Set(now);
            active.update(&state.db).await.map_err(AppError::Database)?;
        } else {
            let new_override = user_persona_override::ActiveModel {
                id: Set(Uuid::new_v4()),
                user_id: Set(auth.id),
                persona_id: Set(base.id),
                greeting: Set(body.greeting),
                behavior_guidelines: Set(body.behavior_guidelines),
                personality_traits: Set(body.personality_traits),
                voice_config: Set(body.voice_config),
                created_at: Set(now),
                updated_at: Set(now),
            };
            new_override
                .insert(&state.db)
                .await
                .map_err(AppError::Database)?;
        }
    }

    // Re-fetch effective persona
    get_my_persona(State(state), auth).await
}

/// Helper: find the user's active persona or fallback to first system persona
async fn find_user_active_persona(
    state: &AppState,
    user_id: Uuid,
) -> Result<persona::Model> {
    // Check explicit selection
    if let Some(active) = user_active_persona::Entity::find_by_id(user_id)
        .one(&state.db)
        .await
        .map_err(AppError::Database)?
    {
        if let Some(p) = persona::Entity::find_by_id(active.persona_id)
            .filter(persona::Column::Enabled.eq(true))
            .one(&state.db)
            .await
            .map_err(AppError::Database)?
        {
            return Ok(p);
        }
    }

    // Fallback: first enabled system persona
    persona::Entity::find()
        .filter(persona::Column::Enabled.eq(true))
        .filter(persona::Column::IsSystem.eq(true))
        .order_by_asc(persona::Column::SortOrder)
        .one(&state.db)
        .await
        .map_err(AppError::Database)?
        .ok_or_else(|| AppError::NotFound("No persona available".into()))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/personas", get(list_personas).post(create_persona))
        .route("/personas/me", get(get_my_persona).patch(update_my_persona))
        .route(
            "/personas/{id}",
            get(get_persona).put(update_persona).delete(delete_persona),
        )
}
