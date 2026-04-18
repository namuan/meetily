# Outbound Network Calls Audit

This document summarizes the outbound network calls identified in the current Meetily repository after telemetry and auto-update removal.

## Scope

This audit covers:

- Direct outbound HTTP(S) endpoints hardcoded in the repository
- Localhost network calls between app components
- Optional user-configured remote AI calls
- Model downloads, and external-link opens

This audit does **not** claim to be a packet capture. It is a code-level inventory of observed egress paths.

## Executive Summary

Meetily is **not network-silent**, but the application code no longer includes built-in telemetry or auto-update checks.

The repository currently contains:

- **Local service calls** to the bundled/local backend and local transcription/model services
- **Optional user-configured OpenAI-compatible endpoint calls** when a custom endpoint is configured
- **Model and dependency downloads**
- **User-initiated external link opens**

## 1. Telemetry

No built-in telemetry endpoints are present in the active frontend or Tauri runtime.

## 2. Localhost / local service traffic

These are network calls, but they stay on the local machine unless the configured local service is itself remote.

### Local transcription stream / whisper server

- **Endpoint**: `http://127.0.0.1:8178` / `http://127.0.0.1:8178/stream`
- **Purpose**: Local transcription service path
- **Source files**:
  - `frontend/src/components/Sidebar/SidebarProvider.tsx`
  - `frontend/src-tauri/tauri.conf.json`

## 3. Optional remote AI calls

These calls occur only when a user supplies and selects a remote OpenAI-compatible endpoint.

### Custom OpenAI-compatible endpoint

- **Endpoint**: User-supplied
- **Purpose**:
  - Connection tests
  - Summary generation
- **Source files**:
  - `frontend/src-tauri/src/api/api.rs`
  - `frontend/src-tauri/src/summary/llm_client.rs`

## 4. Model and binary downloads

### Whisper model downloads

- **Host**: `https://huggingface.co`
- **Purpose**: Download Whisper model files
- **Source file**:
  - `frontend/src-tauri/src/whisper_engine/whisper_engine.rs`

### Parakeet model downloads

- **Host**: `https://huggingface.co`
- **Purpose**: Download Parakeet model assets
- **Source file**:
  - `frontend/src-tauri/src/parakeet_engine/parakeet_engine.rs`

## 5. CSP / allowlisted network destinations

The Tauri CSP in `frontend/src-tauri/tauri.conf.json` explicitly allows:

- `http://localhost:8178`

This is an allowlist, not proof that all of these are actively used in every runtime path. It does, however, show intended network destinations.

## 6. Summary-provider note

The codebase still supports outbound summary-provider calls to:

- A user-configured OpenAI-compatible endpoint

No telemetry implementation was identified during this audit.

## 7. Bottom line

The codebase currently supports or performs outbound calls in these categories:

1. **Local app/service traffic**: localhost transcription server
2. **Optional remote AI calls**: user-configured custom OpenAI-compatible endpoint
3. **Downloads**: Whisper, Parakeet

If the goal is a fully offline / zero-egress build, these areas would need to be disabled, removed, or made opt-in with safer defaults.
