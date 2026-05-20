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

The `.anim` files keep the Python format:

- first line: blendshape count/header, currently ignored by Rust
- following lines: whitespace-separated numbers
- Rust reads the first three values per frame as `[rot_x, rot_y, rot_z]`

## Expression Action

The native motion runtime supports bounded animation playback through the
generic embedded expression service:

```json
{
  "id": "demo-neck-animation",
  "timeline": [
    {
      "action": "play_animation",
      "duration_ms": 1200,
      "params": {
        "name": "neck",
        "fps": 30,
        "loops": 1
      }
    }
  ]
}
```

`name` may be `neck`, `neck2`, or an explicit `.anim` file name inside
`animation_dir`. Paths are rejected intentionally; animations must live in the
configured asset directory.

## Delete Boundary

After the migrated files above are present and the embedded config points to
them, `neck_control` is no longer needed for Rust runtime motion, face tracking,
or animation playback. Keep a backup only if you still need the old Python web
calibration UI or Windows calibration tools.
