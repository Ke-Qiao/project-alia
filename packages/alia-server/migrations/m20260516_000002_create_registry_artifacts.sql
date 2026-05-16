-- Migration: Registry ownership and dependency index.
--
-- Tools and skills are concrete DB rows. This registry index records who owns
-- each artifact and what other artifacts it depends on, so plugin/component
-- removal can be validated before rows are deleted.

CREATE TABLE registry_artifacts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    artifact_kind VARCHAR(20) NOT NULL,          -- 'skill' | 'tool'
    artifact_name VARCHAR(100) NOT NULL,
    owner_kind VARCHAR(50) NOT NULL,             -- 'builtin' | 'file' | 'api' | 'plugin' | 'component'
    owner_name VARCHAR(100) NOT NULL,
    source VARCHAR(100) NOT NULL,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE (artifact_kind, artifact_name)
);

CREATE INDEX idx_registry_artifacts_owner
    ON registry_artifacts(owner_kind, owner_name);

CREATE TABLE registry_dependencies (
    dependent_kind VARCHAR(20) NOT NULL,
    dependent_name VARCHAR(100) NOT NULL,
    dependency_kind VARCHAR(20) NOT NULL,
    dependency_name VARCHAR(100) NOT NULL,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    PRIMARY KEY (dependent_kind, dependent_name, dependency_kind, dependency_name),
    FOREIGN KEY (dependent_kind, dependent_name)
        REFERENCES registry_artifacts(artifact_kind, artifact_name)
        ON DELETE CASCADE,
    FOREIGN KEY (dependency_kind, dependency_name)
        REFERENCES registry_artifacts(artifact_kind, artifact_name)
        ON DELETE RESTRICT
);

CREATE INDEX idx_registry_dependencies_dependency
    ON registry_dependencies(dependency_kind, dependency_name);

INSERT INTO registry_artifacts (
    artifact_kind,
    artifact_name,
    owner_kind,
    owner_name,
    source
)
SELECT
    'skill',
    name,
    CASE
        WHEN source IN ('builtin', 'file', 'api') THEN source
        WHEN source LIKE 'component:%' THEN 'component'
        ELSE 'plugin'
    END,
    CASE
        WHEN source LIKE 'component:%' THEN substring(source FROM 11)
        ELSE source
    END,
    source
FROM skills
ON CONFLICT (artifact_kind, artifact_name) DO UPDATE SET
    owner_kind = EXCLUDED.owner_kind,
    owner_name = EXCLUDED.owner_name,
    source = EXCLUDED.source,
    updated_at = NOW();

INSERT INTO registry_artifacts (
    artifact_kind,
    artifact_name,
    owner_kind,
    owner_name,
    source
)
SELECT
    'tool',
    name,
    CASE
        WHEN source IN ('builtin', 'file', 'api', 'manual') THEN source
        WHEN source LIKE 'component:%' THEN 'component'
        ELSE 'plugin'
    END,
    CASE
        WHEN source LIKE 'component:%' THEN substring(source FROM 11)
        ELSE source
    END,
    source
FROM tools
ON CONFLICT (artifact_kind, artifact_name) DO UPDATE SET
    owner_kind = EXCLUDED.owner_kind,
    owner_name = EXCLUDED.owner_name,
    source = EXCLUDED.source,
    updated_at = NOW();

INSERT INTO registry_dependencies (
    dependent_kind,
    dependent_name,
    dependency_kind,
    dependency_name
)
SELECT
    'skill',
    s.name,
    'tool',
    t.name
FROM skill_tools st
JOIN skills s ON s.id = st.skill_id
JOIN tools t ON t.id = st.tool_id
ON CONFLICT DO NOTHING;
