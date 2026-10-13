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

## Calibration and lifecycle

`motion.calibration_confirmed` defaults to `false`. Set it only after the local
operator has calibrated the actual mechanism, channel mapping, PWM range and
center target. The shipped historical assets and a successfully parsed matrix
are not evidence that this has happened. This confirmation belongs to the local
configuration; a remote Body command cannot supply or bypass it.

With confirmation absent, a mock runtime remains unavailable and issues no
startup targets. Enabling hardware without confirmation is rejected before
backend construction. Once confirmed, the existing matrix, channel count, limits
and center target must still validate. Startup submits the center target once;
only a complete submission establishes the writer's previous software target.
This open-loop startup does not know the actual starting position and does not
prove a physically speed-limited homing operation. Later expression and face
frames retain their shared software speed limit.

Stop, control-lease close/expiry and binding revocation cancel further targets;
they never append a center or output-disable command. An entered native write
remains owned until it returns, even if a stop waiter is dropped. Dropping an RPC
observer does not revoke unrelated device work. Continued PWM, mechanical hold,
power-loss behavior and actual position require separate hardware evidence.

The old `soft_start_enabled`, `soft_start_duration_ms`, `reset_on_stop` and
`reset_duration_ms` keys have been removed. Motion configuration rejects them
instead of silently accepting an ineffective or contradictory setting. Remove
these keys when replacing a local configuration; keep `hardware_enabled=false`
and `calibration_confirmed=false` until actual calibration is confirmed. This
change does not edit deployed configurations or certify the sample assets.

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
