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
approved_animations = []
```

Paths are resolved relative to the embedded config file.

The animation catalog is empty by default. A local operator must review the
exact asset for the selected mechanism, then configure the same canonical
`.anim` filename, positive version, lowercase SHA-256 and byte count on both
server and device. Computing a digest is not approval; shipped historical files
are not automatically added. Each catalog holds at most 64 unique filenames.
The local catalog is loaded at startup. Remote settings may preserve it in an
unchanged snapshot, but cannot add, replace, reorder or remove its entries.
To change approvals, stop the affected processes, edit both controlled local
catalogs, then restart. A running settings manager retains its startup snapshot;
editing its file while it is running can be overwritten by a later ordinary
settings save.

This catalog boundary protects the Body and Settings data paths. It does not
isolate host files from an operator or plugin authorized to execute code as the
embedded process user. The existing plugin deployment and process permissions
are a separate boundary; see [configuration](configuration.md#runtime-state).

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
`alia.body.v1.BodyControl/Exchange` command. The service route remains unchanged;
the JSON Body contract and signing domains are version 2. Version 1 and old
filename-only animation commands are rejected. The old scope-free generic
expression execution path is disabled. An action has the following schematic
shape; the asset fields must come from the operator-reviewed catalog:

```json
{
  "duration_ms": 1200,
  "intensity": 0.2,
  "motion": {
    "kind": "animation",
    "asset": {
      "file": "reviewed.anim",
      "version": 1,
      "sha256": "REPLACE_WITH_OPERATOR_APPROVED_64_LOWERCASE_HEX_DIGEST",
      "bytes": 1234
    },
    "fps": 30,
    "loops": 1
  }
}
```

This is an action fragment, not a complete executable request. The original
Response/owner, voice or nonvoice scope, boot/control lease, sequence, binding
and signatures are required; see [the body contract](../../../docs/refactor/body-command-design.md).
The signature binds the complete asset pin and playback parameters. The device
first requires an exact match with its local catalog, then checks byte count and
SHA-256 against the same bounded buffer that it parses. Invalid UTF-8, frame
angles, source count, loops or duration remain rejected before admission.

`asset.file` must be the canonical single-component name ending in `.anim`;
the signed command cannot use the old extensionless alias. On macOS/Linux, the
reader opens a directory descriptor and opens the leaf relative to it with
no-follow/nonblocking flags, then requires a regular file. Symlinks in either
the final configured directory component or the leaf are rejected. Intermediate
configuration path components remain under local operator control. Renaming a
directory after its descriptor opens cannot redirect the read; replacing a file
after it opens cannot substitute a second path lookup. In-place changes must
still match the approved bytes. Unsupported platforms fail closed for animation
asset opening rather than use an unsafe fallback.

Preparation retains the parsed trajectory in memory; execution never reopens
the file. A later replacement affects new preparations, not the already verified
trajectory. Expired/revoked authority still blocks admission and individual
native writes. Four owned preparation workers remain bounded and joined during
shutdown, even after an RPC times out or its waiter is dropped. A failed repeat
preparation cannot replace an existing receipt with a false zero-write claim.

Content identity does not certify mechanism safety. Keep hardware output disabled
and calibration unconfirmed until actual local calibration and hardware checks
are complete; the historical files remain unapproved reference assets.

## Delete Boundary

After the migrated files above are present and the embedded config points to
them, `neck_control` is no longer needed for Rust runtime motion, face tracking,
or animation playback. Keep a backup only if you still need the old Python web
calibration UI or Windows calibration tools.
