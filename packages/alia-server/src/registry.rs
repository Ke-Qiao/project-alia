//! Runtime registry ownership and dependency helpers.
//!
//! This module keeps plugin/component lifecycle rules out of the individual
//! providers. A skill/tool row says what the artifact is; the registry says who
//! owns it and which other artifacts depend on it.

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    Set,
};
use uuid::Uuid;

use crate::entity::{registry_artifact, registry_dependency, skill, tool};

pub const KIND_SKILL: &str = "skill";
pub const KIND_SENSOR: &str = "sensor";
pub const KIND_TOOL: &str = "tool";

#[derive(Clone)]
pub struct ArtifactRegistry {
    db: DatabaseConnection,
}

impl ArtifactRegistry {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub async fn upsert_artifact(
        &self,
        artifact: &ArtifactRef,
        source: &str,
    ) -> anyhow::Result<()> {
        self.upsert_artifact_in(&self.db, artifact, source).await
    }

    pub async fn upsert_artifact_in(
        &self,
        db: &impl ConnectionTrait,
        artifact: &ArtifactRef,
        source: &str,
    ) -> anyhow::Result<()> {
        upsert_artifact(db, artifact, source).await
    }

    pub async fn delete_artifact(&self, artifact: &ArtifactRef) -> anyhow::Result<u64> {
        self.delete_artifact_in(&self.db, artifact).await
    }

    pub async fn delete_artifact_in(
        &self,
        db: &impl ConnectionTrait,
        artifact: &ArtifactRef,
    ) -> anyhow::Result<u64> {
        delete_artifact(db, artifact).await
    }

    pub async fn replace_dependencies(
        &self,
        dependent: &ArtifactRef,
        dependencies: &[ArtifactRef],
    ) -> anyhow::Result<()> {
        self.replace_dependencies_in(&self.db, dependent, dependencies)
            .await
    }

    pub async fn replace_dependencies_in(
        &self,
        db: &impl ConnectionTrait,
        dependent: &ArtifactRef,
        dependencies: &[ArtifactRef],
    ) -> anyhow::Result<()> {
        replace_dependencies(db, dependent, dependencies).await
    }

    pub async fn ensure_no_external_dependents(
        &self,
        removal_set: &[ArtifactRef],
    ) -> anyhow::Result<()> {
        self.ensure_no_external_dependents_in(&self.db, removal_set)
            .await
    }

    pub async fn ensure_no_external_dependents_in(
        &self,
        db: &impl ConnectionTrait,
        removal_set: &[ArtifactRef],
    ) -> anyhow::Result<()> {
        ensure_no_external_dependents(db, removal_set).await
    }

    pub async fn sync_existing_skill_artifacts(&self) -> anyhow::Result<()> {
        sync_existing_skill_artifacts(&self.db).await
    }

    pub async fn sync_existing_tool_artifacts(&self) -> anyhow::Result<()> {
        sync_existing_tool_artifacts(&self.db).await
    }

    pub async fn owned_artifacts(
        &self,
        owner_kind: &str,
        owner_name: &str,
        artifact_kind: Option<&str>,
    ) -> anyhow::Result<Vec<ArtifactRef>> {
        self.owned_artifacts_in(&self.db, owner_kind, owner_name, artifact_kind)
            .await
    }

    pub async fn owned_artifacts_in(
        &self,
        db: &impl ConnectionTrait,
        owner_kind: &str,
        owner_name: &str,
        artifact_kind: Option<&str>,
    ) -> anyhow::Result<Vec<ArtifactRef>> {
        let mut query = registry_artifact::Entity::find()
            .filter(registry_artifact::Column::OwnerKind.eq(owner_kind))
            .filter(registry_artifact::Column::OwnerName.eq(owner_name));
        if let Some(kind) = artifact_kind {
            query = query.filter(registry_artifact::Column::ArtifactKind.eq(kind));
        }
        Ok(query
            .all(db)
            .await?
            .into_iter()
            .map(|row| ArtifactRef {
                kind: row.artifact_kind,
                name: row.artifact_name,
            })
            .collect())
    }

    pub async fn dependency_tree(
        &self,
        root: ArtifactRef,
        max_depth: usize,
    ) -> anyhow::Result<ArtifactDependencyTree> {
        dependency_tree(&self.db, root, max_depth).await
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ArtifactRef {
    pub kind: String,
    pub name: String,
}

impl ArtifactRef {
    pub fn skill(name: impl Into<String>) -> Self {
        Self {
            kind: KIND_SKILL.to_string(),
            name: name.into(),
        }
    }

    pub fn tool(name: impl Into<String>) -> Self {
        Self {
            kind: KIND_TOOL.to_string(),
            name: name.into(),
        }
    }

    pub fn sensor(name: impl Into<String>) -> Self {
        Self {
            kind: KIND_SENSOR.to_string(),
            name: name.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactOwner {
    pub kind: String,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactDependencyTree {
    pub root: ArtifactRef,
    pub dependencies: Vec<ArtifactDependencyEdge>,
    pub dependents: Vec<ArtifactDependencyEdge>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactDependencyEdge {
    pub from: ArtifactRef,
    pub to: ArtifactRef,
    pub depth: usize,
}

pub fn owner_from_source(source: &str) -> ArtifactOwner {
    if let Some(component) = source.strip_prefix("component:") {
        return ArtifactOwner {
            kind: "component".to_string(),
            name: component.to_string(),
        };
    }

    if let Some(plugin) = source.strip_prefix("plugin:") {
        return ArtifactOwner {
            kind: "plugin".to_string(),
            name: plugin.to_string(),
        };
    }

    match source {
        "builtin" | "file" | "api" | "manual" => ArtifactOwner {
            kind: source.to_string(),
            name: source.to_string(),
        },
        "grpc" | "http" | "rules" | "scheduler" | "sensor" => ArtifactOwner {
            kind: "component".to_string(),
            name: source.to_string(),
        },
        plugin => ArtifactOwner {
            kind: "plugin".to_string(),
            name: plugin.to_string(),
        },
    }
}

pub fn is_plugin_source(source: &str) -> bool {
    owner_from_source(source).kind == "plugin"
}

async fn upsert_artifact(
    db: &impl ConnectionTrait,
    artifact: &ArtifactRef,
    source: &str,
) -> anyhow::Result<()> {
    let owner = owner_from_source(source);
    let now = Utc::now();

    let existing = registry_artifact::Entity::find()
        .filter(registry_artifact::Column::ArtifactKind.eq(&artifact.kind))
        .filter(registry_artifact::Column::ArtifactName.eq(&artifact.name))
        .one(db)
        .await?;

    match existing {
        Some(row) => {
            let mut active: registry_artifact::ActiveModel = row.into();
            active.owner_kind = Set(owner.kind);
            active.owner_name = Set(owner.name);
            active.source = Set(source.to_string());
            active.updated_at = Set(now);
            active.update(db).await?;
        }
        None => {
            registry_artifact::ActiveModel {
                id: Set(Uuid::new_v4()),
                artifact_kind: Set(artifact.kind.clone()),
                artifact_name: Set(artifact.name.clone()),
                owner_kind: Set(owner.kind),
                owner_name: Set(owner.name),
                source: Set(source.to_string()),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(db)
            .await?;
        }
    }

    Ok(())
}

async fn delete_artifact(db: &impl ConnectionTrait, artifact: &ArtifactRef) -> anyhow::Result<u64> {
    let result = registry_artifact::Entity::delete_many()
        .filter(registry_artifact::Column::ArtifactKind.eq(&artifact.kind))
        .filter(registry_artifact::Column::ArtifactName.eq(&artifact.name))
        .exec(db)
        .await?;
    Ok(result.rows_affected)
}

async fn replace_dependencies(
    db: &impl ConnectionTrait,
    dependent: &ArtifactRef,
    dependencies: &[ArtifactRef],
) -> anyhow::Result<()> {
    ensure_artifact_exists(db, dependent).await?;

    for dependency in dependencies {
        ensure_artifact_exists(db, dependency).await?;
    }

    registry_dependency::Entity::delete_many()
        .filter(registry_dependency::Column::DependentKind.eq(&dependent.kind))
        .filter(registry_dependency::Column::DependentName.eq(&dependent.name))
        .exec(db)
        .await?;

    let now = Utc::now();
    let mut seen = HashSet::new();
    for dependency in dependencies {
        if !seen.insert((dependency.kind.as_str(), dependency.name.as_str())) {
            continue;
        }

        registry_dependency::ActiveModel {
            dependent_kind: Set(dependent.kind.clone()),
            dependent_name: Set(dependent.name.clone()),
            dependency_kind: Set(dependency.kind.clone()),
            dependency_name: Set(dependency.name.clone()),
            created_at: Set(now),
        }
        .insert(db)
        .await?;
    }

    Ok(())
}

async fn ensure_no_external_dependents(
    db: &impl ConnectionTrait,
    removal_set: &[ArtifactRef],
) -> anyhow::Result<()> {
    let removal = removal_set
        .iter()
        .cloned()
        .collect::<HashSet<ArtifactRef>>();

    for artifact in removal_set {
        let dependents = registry_dependency::Entity::find()
            .filter(registry_dependency::Column::DependencyKind.eq(&artifact.kind))
            .filter(registry_dependency::Column::DependencyName.eq(&artifact.name))
            .all(db)
            .await?;

        for edge in dependents {
            let dependent = ArtifactRef {
                kind: edge.dependent_kind,
                name: edge.dependent_name,
            };
            if !removal.contains(&dependent) {
                anyhow::bail!(
                    "cannot remove {} '{}': still required by {} '{}'",
                    artifact.kind,
                    artifact.name,
                    dependent.kind,
                    dependent.name
                );
            }
        }
    }

    Ok(())
}

async fn sync_existing_skill_artifacts(db: &impl ConnectionTrait) -> anyhow::Result<()> {
    let rows = skill::Entity::find().all(db).await?;
    for row in rows {
        upsert_artifact(db, &ArtifactRef::skill(row.name), &row.source).await?;
    }
    Ok(())
}

async fn sync_existing_tool_artifacts(db: &impl ConnectionTrait) -> anyhow::Result<()> {
    let rows = tool::Entity::find().all(db).await?;
    for row in rows {
        upsert_artifact(db, &ArtifactRef::tool(row.name), &row.source).await?;
    }
    Ok(())
}

async fn dependency_tree(
    db: &impl ConnectionTrait,
    root: ArtifactRef,
    max_depth: usize,
) -> anyhow::Result<ArtifactDependencyTree> {
    let dependencies =
        walk_dependencies(db, root.clone(), max_depth, Direction::Dependencies).await?;
    let dependents = walk_dependencies(db, root.clone(), max_depth, Direction::Dependents).await?;
    Ok(ArtifactDependencyTree {
        root,
        dependencies,
        dependents,
    })
}

async fn ensure_artifact_exists(
    db: &impl ConnectionTrait,
    artifact: &ArtifactRef,
) -> anyhow::Result<()> {
    let exists = registry_artifact::Entity::find()
        .filter(registry_artifact::Column::ArtifactKind.eq(&artifact.kind))
        .filter(registry_artifact::Column::ArtifactName.eq(&artifact.name))
        .one(db)
        .await?
        .is_some();

    if !exists {
        anyhow::bail!("unknown {} dependency '{}'", artifact.kind, artifact.name);
    }

    Ok(())
}

#[derive(Clone, Copy)]
enum Direction {
    Dependencies,
    Dependents,
}

async fn walk_dependencies(
    db: &impl ConnectionTrait,
    root: ArtifactRef,
    max_depth: usize,
    direction: Direction,
) -> anyhow::Result<Vec<ArtifactDependencyEdge>> {
    let mut edges = Vec::new();
    let mut frontier = vec![(root, 0usize)];
    let mut seen = HashMap::<ArtifactRef, usize>::new();

    while let Some((current, depth)) = frontier.pop() {
        if depth >= max_depth {
            continue;
        }
        let rows = match direction {
            Direction::Dependencies => {
                registry_dependency::Entity::find()
                    .filter(registry_dependency::Column::DependentKind.eq(&current.kind))
                    .filter(registry_dependency::Column::DependentName.eq(&current.name))
                    .all(db)
                    .await?
            }
            Direction::Dependents => {
                registry_dependency::Entity::find()
                    .filter(registry_dependency::Column::DependencyKind.eq(&current.kind))
                    .filter(registry_dependency::Column::DependencyName.eq(&current.name))
                    .all(db)
                    .await?
            }
        };

        for row in rows {
            let (from, to, next) = match direction {
                Direction::Dependencies => {
                    let dependency = ArtifactRef {
                        kind: row.dependency_kind,
                        name: row.dependency_name,
                    };
                    (current.clone(), dependency.clone(), dependency)
                }
                Direction::Dependents => {
                    let dependent = ArtifactRef {
                        kind: row.dependent_kind,
                        name: row.dependent_name,
                    };
                    (dependent.clone(), current.clone(), dependent)
                }
            };
            let next_depth = depth + 1;
            if seen.get(&next).is_some_and(|known| *known <= next_depth) {
                continue;
            }
            seen.insert(next.clone(), next_depth);
            edges.push(ArtifactDependencyEdge {
                from,
                to,
                depth: next_depth,
            });
            frontier.push((next, next_depth));
        }
    }

    Ok(edges)
}
