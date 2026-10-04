# Neck Motion Assets

The Python `neck_control` runtime assets have been copied into the embedded
package so `alia-embedded` no longer needs the Python folder at runtime.

## Tracked Assets

```text
packages/alia-embedded/assets/neck/
├── anim_data/
│   ├── neck.anim
│   └── neck2.anim
└── controller/
    ├── face_calibration_config.json
    ├── neck_config.json
    ├── neck_matrix_config.json
    └── web_neck_config.json
```

`neck_matrix_config.json` is the active Rust neck mapper input. The other JSON
files are preserved as calibration/reference assets from the Python toolchain.

## Configuration

`config/embedded.example.toml` points at the migrated assets:

```toml
[motion]
neck_config_file = "../packages/alia-embedded/assets/neck/controller/neck_matrix_config.json"
animation_dir = "../packages/alia-embedded/assets/neck/anim_data"
```

Paths are resolved relative to the embedded config file.

## Animation Format

The `.anim` reader requires:

- first line: blendshape count/header, currently ignored by Rust
- following nonempty lines: exactly three finite whitespace-separated numbers
  as `[rot_x, rot_y, rot_z]`, each within `[-180, 180]`
- at most 64 KiB per file and 600 source frames; loops and playback cadence
  must fit the declared bounded action duration

## Expression Action

The native motion runtime accepts animation actions inside the signed
`alia.body.v1.BodyControl/Exchange` command. The old scope-free generic
expression execution path is disabled. An animation action has this shape:

```json
{
  "duration_ms": 1200,
  "intensity": 0.2,
  "motion": { "kind": "animation", "file": "neck", "fps": 30, "loops": 1 }
}
```

This is an action fragment, not a complete executable request. The original
Response/owner, voice or nonvoice scope, boot/control lease, sequence, binding
and signatures are required; see [the body contract](../../../docs/refactor/body-command-design.md).
The file must pass the current size/frame/duration validation before admission.

`file` may be `neck`, `neck2`, or an explicit `.anim` file name inside
`animation_dir`. Paths and symlink escapes are rejected; animations must be
regular files within the configured asset directory. The command signature
covers the file name and playback parameters, not the file contents. Local
asset signatures/hashes and validation against the selected mechanism remain
R03 work; these historical files are not evidence of hardware safety.

## Delete Boundary

After the migrated files above are present and the embedded config points to
them, `neck_control` is no longer needed for Rust runtime motion, face tracking,
or animation playback. Keep a backup only if you still need the old Python web
calibration UI or Windows calibration tools.
