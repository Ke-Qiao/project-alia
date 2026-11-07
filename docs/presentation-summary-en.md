# Affective Companionship and Smart Home Hub Device

> Status update, 2026-11-08: This presentation describes the system before frontend removal. The previous frontend is now deleted; WebView, Swift and Kotlin replacements are planned but not implemented. See the [README](../README.md#planned-frontends).

## 1. Slide 1: Background and Objectives

### Project Title

**Affective Companionship and Smart Home Hub Device**

### Project Background

With the increasing prevalence of single-person households, remote study, and home-based work, conventional smart speakers and smart home gateways are often limited to a command-response interaction model. Although they can execute voice commands, they generally lack long-term memory, persistent personality, and embodied affective feedback. As a result, each interaction tends to feel isolated and transactional.

This project aims to integrate an intelligent voice assistant, smart home control, and embodied affective companionship into a unified device. Rather than functioning merely as a passive command-execution terminal, the system is designed as a domestic companion that can remember user preferences, interpret contextual cues, and respond through both speech and physical motion.

### Core Problem

The central problem addressed by this project is:

**The lack of conversational companionship in single-person living environments, combined with the absence of long-term memory and affective continuity in existing smart home systems.**

More specifically, current systems often exhibit the following limitations:

- Conventional smart home systems mainly process immediate commands and do not develop a persistent understanding of the user.
- Voice assistants lack embodied feedback, making the interaction feel like operating a control panel rather than engaging with a companion.
- Most existing systems do not integrate voice interaction, memory, affective expression, motion control, and home automation within a coherent and extensible architecture.
- Domestic environments require low latency, interruption handling, privacy protection, safety constraints, and device-level reliability, which makes it unsuitable to allow cloud-based generative models to directly control hardware.

### Project Objectives

The objective of this project is to develop an **anime-style movable bust device** that provides intelligent companionship and smart home hub capabilities. The system is designed to support the following functions:

- **Intelligent voice interaction**: wake-word detection, speech recognition, LLM-based dialogue, speech synthesis, and continuous conversation.
- **Smart home control**: integration with tools, sensors, rules, and scheduling mechanisms for device control and home automation.
- **Affective companionship**: long-term memory, relationship state modeling, expression planning, and adaptive speech styles.
- **Personal assistant functions**: task execution, information retrieval, reminders, device status queries, and user preference management.
- **Embodied expressive motion**: a cable-driven neck mechanism that enables nodding, gaze orientation, bowing, and other subtle companion-like gestures.

A concise objective statement for presentation use:

> This project develops an anime-style desktop bust with long-term memory, natural voice interaction, smart home control, and embodied affective expression, transforming the smart home interface from a command tool into a companion-like personal assistant.

## 2. Slide 2: Conceptual Design

### Overall Architecture

The project adopts a layered architecture consisting of a **central server, an embedded edge node, a frontend configuration application, and a movable bust hardware platform**.

```text
User speech / Sensors / Frontend settings
        ↓
Embedded edge node:
Wake word, VAD, audio capture, playback, motion execution, sensors
        ↓ gRPC
Server:
STT, LLM, memory, skills, tools, automation, expression planning, TTS
        ↓
Voice response + motion plan + smart home control result
        ↓
Movable anime-style bust:
speech, gaze, nodding, bowing, affective feedback
```

This design assigns latency-sensitive tasks to the local edge node, while heavier reasoning, memory retrieval, tool execution, and multi-module orchestration are handled by the server. The frontend application is responsible for settings, device management, and system observability.

### Primary Design Scheme

| Module | Adopted Design | Function |
| --- | --- | --- |
| Mechanical structure | Anime-style movable bust with a cable-driven neck mechanism | Enables compact, soft, and human-like head-neck motion |
| Motion control | RBF matrix mapping with taught posture samples | Maps target postures to servo angles without explicit inverse kinematics |
| Electronic control | Raspberry Pi/Linux edge node, PCA9685 servo control, microphone, speaker, and sensors | Handles audio, motion, sensing, and device connectivity |
| Software system | Rust server, Rust embedded runtime, and Expo/React Native frontend | Provides a reliable, extensible, and observable AI assistant architecture |
| AI voice pipeline | Wake word, VAD, STT, LLM, TTS, and streaming playback | Supports natural speech interaction, continuous dialogue, and barge-in interruption |
| Affective expression | Appraisal → Affect/Social/Resource → Regulation → Expression Plan → Adapter | Decouples affect interpretation, expression intent, and hardware execution |
| Smart home capability | Tools, skills, sensors, rules, and scheduler | Supports device reading, device control, automation, and contextual actions |
| Memory capability | Built-in Alia memory policy and store | Stores durable facts, preferences, goals, and relationship context in a controlled manner |

## 3. Slide 3: Detailed Design and Engineering Advantages

### Mechanical Structure: Cable-Driven Neck Mechanism

The mechanical structure adopts a cable-driven neck mechanism instead of a rigid linkage or a complex robotic arm. This choice is motivated by several engineering considerations:

- **More natural embodied expression**: subtle head-neck motions, such as nodding, bowing, and looking toward the user, are well suited to companion-like interaction.
- **Compact structure**: the internal space of a bust is limited, and cable transmission allows the actuators to be arranged more flexibly.
- **Improved interaction safety**: motion range and torque can be constrained, which is important for close-range desktop interaction.
- **Lower cost and better manufacturability**: the mechanism can be implemented using multiple servos and a PCA9685 controller, making it suitable for iterative prototyping.

The embedded configuration already supports eight servo channels, PCA9685 control, I2C communication, motion frame rate, center angle calibration, face-following angle limits, and smoothing parameters. This indicates that the motion subsystem is designed for deployable hardware rather than a single-purpose demonstration script.

### Control Algorithm: RBF Matrix Mapping

The neck motion controller uses **Radial Basis Function (RBF) matrix mapping**. The input is the target head-neck rotation posture, and the output is a set of servo angles.

Key advantages include:

- **No explicit inverse kinematics required**: cable-driven mechanisms involve friction, elasticity, assembly error, and nonlinear coupling, making analytical inverse kinematics difficult. RBF mapping bypasses this complexity.
- **Postures can be stored through teaching**: the system only needs a set of representative taught postures and their corresponding servo angles to construct the mapping matrix.
- **Suitable for nonlinear mechanisms**: the actual behavior of a cable-driven neck is not purely linear, while Gaussian RBFs can approximate smooth nonlinear mappings.
- **Low iteration cost**: after mechanical adjustments, the system can be recalibrated by teaching key postures rather than deriving a new kinematic model.
- **Lightweight runtime computation**: the embedded node only needs to compute RBF weights and perform matrix-weighted summation, which is suitable for real-time motion control.

A presentation-ready algorithm description:

```text
Target neck posture [pitch, yaw, roll]
        ↓
Compute Gaussian RBF weights against taught postures
        ↓
RBF weights × servo mapping matrix
        ↓
Output 8-channel servo angles
        ↓
Apply clamping, smoothing, and trajectory interpolation
        ↓
Send commands to PCA9685
```

### Software System: Modular AI Assistant Architecture

The software design can be summarized as:

**A reliable Rust-based server, a Rust-based embedded edge runtime, an Expo/React Native frontend, and replaceable model-service adapters.**

The responsibilities are divided as follows:

- `alia-server` manages AI orchestration, user state, memory, tool execution, skill routing, automation rules, STT/TTS/LLM adapters, and persistent storage.
- `alia-embedded` handles local audio I/O, wake-word detection, VAD, direct responses, playback interruption, sensors, device binding, and motion execution.
- `alia-frontend` provides user settings, device management, conversation views, plugin management, sensor inspection, and runtime observability.
- Model services are integrated through provider adapters, allowing the system to switch among OpenAI-compatible APIs, Anthropic, Gemini, whisper.cpp, GPT-SoVITS, and other backends.

### Affective Expression Design

The affective expression system is not implemented as a simple emotion label attached to TTS. Instead, it uses a layered expression runtime:

```text
User input
  → Appraisal
  → Affect / Social / Resource state
  → Regulation
  → Expression Intent Planner
  → Expression Realizer
  → Safety Policy
  → TTS / Motion / Face / Light adapters
```

This design reflects an engineering-oriented approach:

- The LLM participates in high-level semantic reasoning but does not directly control motors.
- Internal affective state is separated from external expression, reducing mismatches such as expressing cheerful motion during serious content.
- Motion execution is preceded by capability negotiation and safety constraints.
- If motion hardware is unavailable, the system can gracefully degrade to speech-only interaction.
- Expression decisions are traceable and replayable, making it easier to debug unnatural behavior.

### Smart Home and Personal Assistant Capabilities

The smart home layer is abstracted through tools, skills, sensors, and automation rules:

- **Tools** are executable actions, such as reading sensors, controlling devices, and storing memories.
- **Skills** provide cognitive context, such as smart home control, memory-aware conversation, and information retrieval.
- **Sensors** are exposed through a unified registry for tools, automation, and frontend inspection.
- **Rules and scheduler tasks** support event-triggered actions, timed tasks, condition polling, and tool invocation.

As a result, the system is not limited to conversation; it can function as a practical smart home hub.

## 4. Project Characteristics and Advantages

### 1. Embodied Companionship Beyond a Voice Box

The project uses an anime-style movable bust as the physical carrier of the assistant persona. Through speech, motion, and expressive intent, the user interacts with a character that can respond, orient toward them, and express attitude through subtle gestures, rather than with a speaker that only plays audio.

### 2. Long-Term Companionship Driven by Memory

The system supports user memories, preferences, relationship state, and conversation history. It does not restart from a blank state in every interaction, but can adapt responses and expressions based on stable preferences and long-term user context.

### 3. RBF Teaching-Based Control for Cable-Driven Motion

Because cable-driven neck mechanisms are nonlinear and difficult to model analytically, RBF mapping provides a practical solution. It directly learns the relationship between taught postures and servo angles, reducing modeling complexity and improving calibration efficiency.

### 4. Edge Runtime for Responsive Interaction

Wake-word detection, VAD, direct responses, playback interruption, and motion execution are handled locally. This allows the device to provide immediate feedback even when server-side reasoning takes longer, and enables natural barge-in behavior when the user interrupts the assistant.

### 5. Clear Separation Between Intelligence and Hardware Control

The server handles LLM reasoning, memory, tools, automation, and expression planning, while the embedded node handles audio, servos, sensors, and playback. This separation gives the system access to advanced model capabilities without exposing real-time hardware directly to generative model outputs.

### 6. Modular Provider Architecture

STT, LLM, TTS, embedding, and memory functions are integrated through provider adapters. This makes it possible to switch model services according to cost, latency, privacy, or deployment requirements without rewriting the core runtime.

### 7. Safe and Controllable Affective Expression

The system includes expression regulation, motion allowlists, intensity limits, duration limits, night-mode behavior, and degradation under low-battery or thermal constraints. The device can express affect while remaining bounded and safe.

### 8. Reliability for Real Home Environments

The architecture follows a degradation-first failure model. If a model service, sensor, or motion device becomes unavailable, the system attempts to degrade the affected function instead of crashing the whole process. This reliability is essential for domestic devices and distinguishes the system from a one-off demonstration prototype.

## 5. Condensed Text for Slides

### Slide 1: Background and Objectives

**Project title**: Affective Companionship and Smart Home Hub Device

**Core problem**: Single-person living environments lack natural conversational companionship, while existing smart home systems lack long-term memory and affective continuity.

**Objective**: To develop an anime-style movable bust device that integrates voice interaction, smart home control, long-term memory, affective companionship, and personal assistant functions.

### Slide 2: Conceptual Design

The project adopts a layered architecture consisting of a central server, an embedded edge node, a frontend configuration application, and movable bust hardware. The edge node handles wake-word detection, VAD, audio playback, sensors, and motion control. The server handles STT, LLM, TTS, memory, tool execution, automation, and expression planning. The frontend provides settings, device management, and runtime observability.

### Slide 3: Detailed Design

The mechanical system uses a cable-driven neck mechanism, while the motion controller adopts RBF matrix mapping. By teaching key postures and corresponding servo angles, the system can compute head-neck motion without explicit inverse kinematics. The software system combines a Rust server, Rust embedded runtime, Expo frontend, replaceable model services, memory, skills/tools, and a safe expression runtime to achieve an extensible, low-latency, and debuggable smart companionship hub.

## 6. Recommended Slide Titles

- Slide 1: **Background and Objectives**
- Slide 2: **System Concept**
- Slide 3: **Mechanical, Control, and Software Design**
