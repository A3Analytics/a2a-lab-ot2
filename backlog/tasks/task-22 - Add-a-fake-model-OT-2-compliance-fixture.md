---
id: TASK-22
title: Add a fake-model OT-2 compliance fixture
status: Done
assignee:
  - '@me'
created_date: '2026-10-09 15:59'
updated_date: '2026-10-09 16:26'
labels:
  - compliance
  - backlog-implementer
dependencies:
  - TASK-21
references:
  - task-21
  - src/agent.rs
  - tests/agent.rs
  - src/fixture.rs
priority: high
type: feature
ordinal: 22000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Exercise the OT-2 application's actual LLM-enabled architecture in deterministic compliance runs by wiring a fake completion model through LabAgent and its MCP tools. The fixture must prove the A2A agent-message to model to MCP-tool path without external credentials, while remaining isolated from production provider configuration.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The standard compliance fixture advertises the A2A agent-message skill and routes its handler through LabAgent connected to the fixture-owned MCP endpoint
- [x] #2 A deterministic fake completion model issues the expected MCP list_tasks tool call and returns a reply containing the exact task identifier supplied by the full-suite LLM scenario
- [x] #3 The fixture starts successfully with empty or invalid Bedrock, OpenAI, Anthropic, AWS, and model environment configuration and makes no external model-provider network request
- [x] #4 Conversation persistence used by the fake-model path is fixture-owned, isolated between runs, and removed during normal shutdown and failure cleanup
- [x] #5 A controlled fixture variant produces an ungrounded or tool-failure reply so the runner records a specific noncompliant LLM scenario while preserving the report
- [x] #6 Focused tests distinguish successful fake-model tool use, the controlled LLM failure, default-on execution, and explicit opt-out with no fake-model invocation
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Wire a fixture-only deterministic completion model through LabAgent and the fixture-owned MCP endpoint, with standard and controlled-failure behavior.
2. Own conversation persistence in a per-run temporary directory and remove it on startup failure, shutdown, and drop.
3. Enable the full-suite LLM scenario by default, add focused success/failure/opt-out/environment/cleanup tests, then run mise run quality and complete verified acceptance criteria.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- Wired the standard fixture A2A server to LabAgent using a deterministic local completion model and the fixture-owned MCP endpoint; the model requires and calls list_tasks, verifies the tool result contains the prompt-supplied task ID, and returns only a bounded grounded reply.
- Added the ungrounded-agent-reply controlled variant. Its full-suite report is preserved as noncompliant with the specific agent-message task-ID diagnostic, while explicit LLM opt-out runs 16 linked scenarios with zero model invocations.
- Added per-run SQLite conversation directories under the system temporary directory and cleanup on startup failure, readiness failure, explicit shutdown, and drop. Invalid model/provider credentials and external service configuration do not affect fixture startup.
- Enabled the LLM scenario by default in mise run compliance and changed fixture process cleanup to request graceful interrupt.
- Verification passed: focused compliance_fixture tests (7/7), mise run compliance with a2a_agent_message enabled and compliant=true, git diff --check, IDE diagnostics, and mise run quality (75/75 tests).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added a deterministic fixture-only completion model that drives the real LabAgent to the fixture-owned MCP list_tasks tool and returns the exact task identifier requested by the profile 1.1.0 full-suite LLM scenario. No provider client or live credential is used.

Conversation state is isolated in a per-run temporary SQLite directory and cleaned up across shutdown, drop, and startup/readiness failures. A controlled ungrounded variant preserves a specific noncompliant report, and explicit opt-out proves zero fake-model invocations. Focused tests, default-on compliance, and mise run quality (75/75 tests) pass.
<!-- SECTION:FINAL_SUMMARY:END -->
