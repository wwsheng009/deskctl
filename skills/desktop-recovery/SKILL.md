---
name: desktop-recovery
description: "Recovery and safety playbook for deskctl desktop automation after a failed, expired, or ambiguous action. Use before retrying anything when a tool returns stale_capture, stale_observation, target_unavailable, resource_busy, dispatch_unknown, dispatch_state=sent/partial/unknown, or model_unavailable. Re-screenshot and re-perceive stale captures, re-bind stale targets, back off on resource_busy, and observe the UI after unknown dispatch; never replay a click or keystroke, reuse stale candidate IDs, or guess coordinates. Use together with desktop-basic, desktop-window, desktop-text, or desktop-semantic; this is not a first-step control skill. Deskctl has no approval flow, so stop and ask the user when the intended outcome is unclear."
---

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
