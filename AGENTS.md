
# AI Agent Guidelines

## Project overview

Comms is a lightweight, low-latency voice communication
application written in Rust.

The project prioritizes:
- Low audio latency
- Low memory and CPU overhead
- Reliable network communication
- Cross-platform compatibility

## Workspace architecture

- `comms/`: Desktop client and GTK user interface
- `comms-server/`: Communication server
- `comms-lib/`: Shared networking, packets and serialization
- `audio-device/`: Platform-specific audio interfaces
- `audio-cli/`: Audio-related CLI
- `loopback-cli/`: Audio loopback utility
- `latency-measurement/`: Latency measurement tools
- `live-noise-gate-ui/`: Noise gate UI

## Development guidelines

- Follow existing Rust conventions.
- Prefer small, focused changes.
- Avoid unnecessary allocations in latency-sensitive paths.
- Avoid blocking operations in real-time audio processing.
- Preserve protocol compatibility unless explicitly requested.
- Do not introduce dependencies without justification.
- Keep platform-specific implementations isolated.

## Protocol compatibility

The shared protocol is defined in `comms-lib`.

- Preserve existing packet kind identifiers.
- Do not change serialization formats without considering
  backward compatibility.
- Add tests for changes to packet encoding or decoding.
- Treat `PROTOCOL_VERSION` changes as explicit decisions.

## Testing and validation

Before completing a change, run the relevant checks:

    cargo fmt --all -- --check
    cargo test -p comms-lib
    cargo test -p comms-server

For wider changes, run:

    cargo test --workspace
    cargo clippy --workspace --all-targets -- -D warnings

Some crates require platform-specific libraries.

If a command cannot run in the current environment,
document the limitation instead of claiming success.

## Contribution workflow

1. Understand the relevant crates before modifying code.
2. Make the smallest change that satisfies the task.
3. Add regression tests for behavior changes.
4. Run formatting and relevant tests.
5. Summarize the changes and verification results.

Do not perform unrelated refactoring.

## Completion requirements

Every contribution should explain:
- What changed and why
- What tests were executed
- Any compatibility implications
- Known limitations or follow-up work