---
id: TASK-8
title: 'Integrate current devkit task, SiLA, and OIDC surfaces'
status: Done
assignee:
  - '@me'
created_date: '2026-10-07 01:53'
updated_date: '2026-10-07 02:03'
labels:
  - integration
dependencies: []
priority: high
ordinal: 8000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sibling devkit now requires task schemas and structured run results, returns application/json, and can serve SiLA and validate OpenID Connect bearer tokens. OT-2 still compiles against the older task literals and does not expose those surfaces.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 list_tasks advertises JSON Schema for composite and primitive HTTP tasks, and the crate type-checks against the sibling devkit with the sila2 feature enabled.
- [x] #2 A completed HTTP task stores robot-server JSON on TaskRun.result, run_serial_dilution reports progress from 0 through 1, and a failed command sets error_kind and error_identifier.
- [x] #3 Default SiLA startup uses UUID 0e2a0002-0000-4000-8000-000000000002 and a self-signed certificate matching that UUID; supplying certificate, key, and CA PEM replaces that certificate.
- [x] #4 With an OpenID Connect issuer configured, A2A rejects a missing bearer token and a token without the a2a.invoke scope, and accepts a token that has that scope. Without an issuer, A2A stays open.
- [x] #5 README and doc-1 describe application/json, the task result fields, the default SiLA endpoint, and opt-in OpenID Connect.
- [x] #6 mise run quality passes.
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Enable the sibling devkit sila2 feature and refresh the lockfile.
2. Advertise JSON Schema on task definitions and fill TaskRun result, progress, and error fields from robot-server responses.
3. Start SiLA by default with the stable OT-2 UUID, self-signed TLS, and operator PEM overrides.
4. Attach the devkit OIDC authenticator to A2A only when an issuer is configured, and send a bearer token from the client when one is supplied.
5. Update README and doc-1, then run mise run quality.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- Enabled the sibling devkit sila2 feature and refreshed Cargo.lock.
- Advertised JSON Schema on composite and HTTP tasks, and filled TaskRun result, progress, and error fields.
- SiLA now starts by default with UUID 0e2a0002-0000-4000-8000-000000000002 and self-signed TLS. Operator PEM files replace that certificate.
- A2A uses the devkit OIDC authenticator only when an issuer is set. MCP and SiLA stay open.
- Updated README, doc-1, and the smoke script. mise run quality passed: 32 tests.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
OT-2 now builds against the current sibling devkit and exposes the new task, SiLA, and OpenID Connect surfaces.

Task definitions advertise JSON Schema. HTTP results land on TaskRun.result, serial dilution reports progress from 0 through 1, and failed commands and runs set error_kind and error_identifier. Serve starts SiLA on 127.0.0.1:50052 with UUID 0e2a0002-0000-4000-8000-000000000002 and a self-signed certificate; certificate, key, and CA PEM files replace it. A2A requires a bearer token only when an OpenID Connect issuer is configured, using the devkit authenticator. MCP and SiLA stay open.

Key files: src/opentrons/provider.rs, src/sila.rs, src/oidc.rs, src/bin/a2a-lab-ot2.rs, tests/provider.rs, tests/sila.rs, tests/a2a.rs, README.md, doc-1.

mise run quality passed (fmt, check, clippy, 32 tests). Live smoke was not run.
<!-- SECTION:FINAL_SUMMARY:END -->
