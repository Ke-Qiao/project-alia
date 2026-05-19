# Appraisal Lexicons

Each language fragment should declare source metadata before phrase lists:

```toml
[resources.appraisal_lexicons.zh.metadata]
source_name = "Project Alia zh appraisal overlay"
source_url = "local"
license = "project"
version = "0.1"
redistributable = true
overlay = true
notes = "Project-owned product cues."
```

Keep project-specific phrases in overlay fragments. Put imported third-party
lexicons in separate fragments with their original source, license, version,
and redistribution status recorded. Do not paste non-redistributable lexicons
into checked-in example config; convert them in deployment-local fragments.
