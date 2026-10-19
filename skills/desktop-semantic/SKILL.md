---
name: desktop-semantic
description: "Preferred Windows UI Automation workflow for deskctl when a stable semantic target is available. Use to list running or launchable targets, bind a window to a target_token, observe UIA candidates for a goal, inspect candidate details, execute a candidate with observation_token + candidate_id, replay a saved workflow_step.params.locator via desktop_semantic_action, and verify postconditions with desktop_verify_state. Requires the standard profile and an active session. Observation tokens are single-use and expire; candidate IDs belong only to the current observation, so re-observe after execution and do not switch windows between observe and execute. Fall back to desktop-basic when UIA is unavailable, and use desktop-recovery after stale or ambiguous failures."
---

# Desktop Semantic

Use Windows UI Automation for target binding, observation, execution and verification.

## Flow

1. `desktop_targets_list`.
2. `desktop_target_bind` to obtain `target_token`.
3. `desktop_semantic_observe` with the target token.
4. `desktop_semantic_candidate` when candidate details are needed.
5. `desktop_semantic_execute` with `observation_token` + `candidate_id`.
6. `desktop_verify_state` to confirm the postcondition.

## Rules

- Prefer semantic tools over coordinate clicks.
- An `observation_token` is single-use and expires; re-observe after execution.
- Do not switch windows between observation and execution.
- Candidate ids belong only to the current observation.
- Replay a saved `workflow_step.params.locator` with `desktop_semantic_action`; never persist temporary tokens.
- `desktop_agent_step` is optional and requires an enhanced decision provider.
- There is no approval flow; the model must judge whether the intent is clear.
