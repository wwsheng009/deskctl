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
- Persist `workflow_step.params.locator`, not temporary tokens.
- `desktop_agent_step` is optional and requires an enhanced decision provider.
- There is no approval flow; the model must judge whether the intent is clear.
