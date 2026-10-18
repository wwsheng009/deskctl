# Desktop Recovery

Use when a desktop action fails, expires or becomes ambiguous.

## Recovery Table

| Error | Action |
|---|---|
| `stale_capture` | Take a fresh screenshot and perceive again |
| `stale_observation` | Re-bind the target and observe again |
| `target_unavailable` | List windows and bind again |
| `resource_busy` | Back off and retry a read operation |
| `dispatch_unknown` | Do not retry the action; observe the result |
| `dispatch_state=sent` | Do not retry; wait and verify the business result |
| `model_unavailable` | Run `deskctl doctor` and check model files |

## Forbidden

- Retrying a click or keystroke after `dispatch_unknown`.
- Reusing a candidate id after the window changed.
- Guessing coordinates to work around a failure.
- Assuming an action succeeded because the call returned.
