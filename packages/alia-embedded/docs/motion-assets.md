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

This catalog boundary protects the Motion and Settings data paths. It does not
isolate host files from an operator or plugin authorized to execute code as the
embedded process user. The existing plugin deployment and process permissions
are a separate boundary; see [configuration](configuration.md#runtime-state).

## Offline configuration and asset check

Use the built binary's explicit inspection mode from the repository root:

```sh
./target/debug/alia-embedded --check-config /absolute/path/embedded.toml
```

This reads an existing configuration and ordinary local motion files; it does
not start a runtime, create identity/state files, load models, open a configured
servo device, or confirm calibration. Missing config is an error. The report's
`hardware_verified` is always false and `motion.physical_state` is
`not_observed`; configured hardware/confirmation flags describe the input, not
the result of this check. Do not use normal startup as a substitute.

The check reuses the runtime's static motion rules. For a selected `pca9685`/`i2c`
backend it checks address range, duplicate addresses and channel coverage even
while hardware is disabled. It checks PWM ordering and the existing positive
frequency condition, not electrical compatibility or measured pulse frequency.
The current driver computes prescale using a nominal 25 MHz clock and clamps its
register/duty calculations; passing this check is not evidence that a requested
frequency or pulse range is correct for the physical board or servo.

An existing matrix is parsed and checked for dimensions, limits, channel count
and configured center even when `calibration_confirmed=false`. The report then
says `calibration.status=checked` without changing that confirmation flag. An
unconfirmed missing matrix yields `not_checked` with reason `missing`; confirmed
missing calibration, malformed files, invalid dimensions or an out-of-limit
center are errors. Final file symlinks, directories and special files are
rejected; calibration reads are bounded to 1 MiB. Local parent directories remain
under operator control, and bounded byte reads do not promise a filesystem
wall-clock deadline.

For each already approved animation, inspection uses the production bounded
read, exact tuple verification and frame parser, then reports the original
`asset` and `source_frames`. It does not add approvals or create a Motion command.
An empty catalog stays empty. This check does not validate a future action's
fps/loops/duration combination, instantiate other modules, compare the Server
catalog, or certify physical motion.

### Preparing a candidate content identity

Select one ordinary `.anim` file for the actual mechanism. Stop any editor or
writer that might change it while reviewing; keep it outside deployed approved
directories until reviewed. On macOS the existing `shasum` and `wc` tools can
show a **candidate** digest and byte count:

```sh
ALIA_MOTION_CANDIDATE=/absolute/path/reviewed.anim
if [ -f "$ALIA_MOTION_CANDIDATE" ] && [ ! -L "$ALIA_MOTION_CANDIDATE" ]; then
  wc -c < "$ALIA_MOTION_CANDIDATE"
  shasum -a 256 -- "$ALIA_MOTION_CANDIDATE"
else
  printf '%s\n' 'Choose an existing regular file, not a symlink.' >&2
fi
```

On Linux, `sha256sum -- "$ALIA_MOTION_CANDIDATE"` is the equivalent digest
command when installed. These two reads are not an atomic snapshot: a changing
file must be reviewed and measured again. Neither command validates frame format
or grants approval. Assign a positive version only as part of the operator's
review record; versions are identifiers, not automatic rollback protection.

After actual content review, stop the affected Server and Embedded processes.
Record the reviewer/date, target mechanism/calibration revision, canonical file,
version, digest and exact byte count. Copy the exact reviewed file to the
controlled Embedded animation directory, then set the same tuple in
`motion.approved_animations` and the intended Server `[[channels]]` entry's
`approved_animations`. Check the exact channel ID and any Server `.d` fragment
overrides; matching only filenames is insufficient. Do not call Server startup
or configuration recovery a read-only catalog comparison.

Run `--check-config` against the installed Embedded configuration after copying:
it verifies the deployed bytes and parser using one bounded buffer. Compare its
reported tuples to the effective Server catalog, including entries missing from
either side. The tool explicitly reports Server catalog agreement as unchecked;
this comparison remains the operator's responsibility. Keep both hardware and
calibration flags false until the separate physical checks below are complete.
Restart only after these gates are satisfied; catalog changes never authorize
replaying an old command.

## Calibration and lifecycle

`motion.calibration_confirmed` defaults to `false`. Set it only after the local
operator has calibrated the actual mechanism, channel mapping, PWM range and
center target. The shipped historical assets and a successfully parsed matrix
are not evidence that this has happened. This confirmation belongs to the local
configuration; a remote Motion command cannot supply or bypass it.

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

### Field calibration record

The selected project hardware is PCA9685A with MG996R, but the example config's
serial backend, eight channels, 50 Hz, 500–2500 µs and 90° center are reference
values, not measured deployment settings. Before enabling output, fill the
following record from the actual mechanism and corresponding board information:

| Required evidence | Record before enabling output |
| --- | --- |
| Board/servo identity | Exact board revision, address jumpers, servo source/model and count; photo or matching documentation |
| Wiring and power | Separate servo supply specification, actual common ground, Pi-to-board wiring, OE connection/default and power sequencing |
| Channel mapping | Each logical channel → board/address/output → physical servo/joint; installation direction and load |
| Calibrated limits | Per-joint measured safe motion/pulse limits, center, matrix file revision and person/date performing calibration |
| Stop/disconnect hold | Last-target behavior under maintained power, control loss, Pi/board restart and power loss; external position observation where needed |
| Deployment | Exact service account, config/asset paths and permissions on both machines; approved tuple comparison record |

The existing mapper and offline checker validate the recorded numbers; they do
not measure or produce this calibration. No maintained Python/Windows calibration
application is provided by this repository. Do not use the historical matrix as
evidence or set `calibration_confirmed=true` just to make an offline report pass.
The agreed behavior remains hold on stop/disconnect and one startup center target
only after actual calibration is confirmed. A host I²C submission is not position
feedback or proof of mechanical hold.

## Animation Format

The `.anim` reader requires:

- first line: blendshape count/header, currently ignored by Rust
- following nonempty lines: exactly three finite whitespace-separated numbers
  as `[rot_x, rot_y, rot_z]`, each within `[-180, 180]`
- at most 64 KiB per file and 600 source frames; loops and playback cadence
  must fit the declared bounded action duration

## Expression Action

The native motion runtime accepts animation actions inside the signed
`alia.motion.v1.MotionControl/Exchange` command. The service route remains unchanged;
the JSON Motion contract and signing domains are version 2. Version 1 and old
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
and signatures are required; see [the Motion contract](../../../docs/architecture/motion-contract.md).
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

## Historical asset source

The files above came from the earlier Python `neck_control` toolchain. That
runtime is no longer required or supplied for Rust motion, face tracking or
animation playback. Its historical calibration/reference files remain available
as source material; they do not constitute a current calibration tool or approval
for the selected mechanism.
