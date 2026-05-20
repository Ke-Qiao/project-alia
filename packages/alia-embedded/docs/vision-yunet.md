# YuNet Vision Setup

`alia-embedded` can replace the Python MediaPipe gaze path with local Rust/OpenCV
face tracking. The tracker uses OpenCV YuNet to detect a face box at low
frequency, takes the primary face center, and feeds it directly into the native
motion runtime.

## Setup Script

On Raspberry Pi 5, from the workspace root:

```sh
./scripts/setup-embedded-vision-pi.sh
```

The script installs Linux build/runtime dependencies, downloads the YuNet ONNX
model, probes `/dev/video0`, and runs:

```sh
cargo check -p alia-embedded --features vision-opencv
```

Useful options:

```sh
./scripts/setup-embedded-vision-pi.sh --camera-index 1
./scripts/setup-embedded-vision-pi.sh --skip-deps
./scripts/setup-embedded-vision-pi.sh --skip-cargo-check
./scripts/setup-embedded-vision-pi.sh --run
```

The default model path is:

```text
config/models/vision/face_detection_yunet_2023mar.onnx
```

The model is downloaded from the OpenCV Zoo YuNet model path:

```text
https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/face_detection_yunet/face_detection_yunet_2023mar.onnx
```

Use the GitHub media URL for this model. The `raw.githubusercontent.com`
OpenCV Zoo path can return a Git LFS pointer file instead of the ONNX payload on
some systems; OpenCV then reports `Failed to parse ONNX model`.

## Manual Dependencies

If the script cannot use `apt-get`, install the equivalent packages manually:

```sh
sudo apt-get update
sudo apt-get install -y \
  build-essential \
  ca-certificates \
  clang \
  cmake \
  curl \
  libclang-dev \
  libopencv-dev \
  pkg-config \
  v4l-utils
```

The Rust `opencv` crate discovers OpenCV through `pkg-config`, CMake, or explicit
environment variables. On Raspberry Pi OS with `libopencv-dev`, this should work:

```sh
pkg-config --modversion opencv4
```

If OpenCV is installed in a custom location, export `PKG_CONFIG_PATH` to the
directory containing `opencv4.pc`.

## Configuration

The relevant defaults live in `[vision]` and `[motion]`:

```toml
[vision]
enabled = true
camera_index = 0
camera_width = 640
camera_height = 480
capture_fps = 30.0
detector_fps = 12
yunet_model_path = "models/vision/face_detection_yunet_2023mar.onnx"
input_width = 320
input_height = 320
score_threshold = 0.75
nms_threshold = 0.3
top_k = 256
min_motion_interval_ms = 80
min_face_delta_px = 6.0

[motion]
face_max_yaw = 25.0
face_max_pitch = 18.0
face_deadzone = 0.04
face_smooth_alpha = 0.2
face_center_offset_x = 0.0
face_center_offset_y = 0.0
face_camera_roll_deg = 0.0
```

`detector_fps = 8..12` and `input_width/input_height = 320` are the intended Pi
5 starting point. If CPU load is too high, lower `detector_fps` first.

## Run

```sh
ALIA_EMBEDDED_CONFIG_PATH=config/embedded.toml \
RUST_LOG=alia_embedded::module::vision=debug,alia_embedded::module::motion=debug,info \
cargo run -p alia-embedded --features vision-opencv
```

Keep `motion.hardware_enabled = false` until the camera path is confirmed. Then
enable hardware and calibrate on the real neck assembly.

## Validation Checklist

- `pkg-config --modversion opencv4` prints an OpenCV version.
- `config/models/vision/face_detection_yunet_2023mar.onnx` exists and is roughly
  230 KB, not a small Git LFS pointer text file.
- `/dev/video0` or the configured camera device exists.
- `cargo check -p alia-embedded --features vision-opencv` passes on the Pi.
- Starting `alia-embedded` logs `yunet_face_tracker` as healthy after camera open.
- With `motion.hardware_enabled = false`, face tracking runs without moving servos.
- With hardware enabled, the neck follows face center without oscillation.

## Tuning Notes

- Increase `face_deadzone` if the head jitters near center.
- Lower `face_smooth_alpha` for smoother but slower following.
- Adjust `face_camera_roll_deg` if the mounted camera is tilted.
- Use `face_center_offset_x/y` when the physical neutral gaze is not exactly the
  camera image center.
- Lower `face_max_yaw/pitch` before testing with real servos, then raise gradually.
