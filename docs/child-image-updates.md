# Child Pod Image Updates

## The Problem

Rulyeh and child containers share a single image (`ghcr.io/georgebradford0/rulyeh`). When a new image is pushed and rulyeh is rolled out, existing child pods are **not** updated — they continue running their old image. A child only picks up a new image when it is stopped and restarted.

## Current Behavior

- `kubectl rollout restart deployment/rulyeh` updates rulyeh only.
- Child pods created before the rollout keep running their original image indefinitely.
- A child gets the new image only when rulyeh creates a fresh pod for it (i.e. after a stop + start).

## Options for Fixing This

### Option A — Rulyeh auto-restarts stale children on startup
On boot, rulyeh compares its own image digest against the image currently running on each child pod. Any child whose image digest differs is restarted automatically.

- **Pro:** Fully automatic, no user action needed.
- **Con:** Disruptive — a child mid-task would be killed without warning.
- **Mitigation:** Only auto-restart children whose status is `stopped`; prompt the user for running ones.

### Option B — Version mismatch surfaced in the UI
Rulyeh reports its own image version alongside each container in the `/containers` response. The mobile client compares and shows an "update available" indicator. The user restarts the child manually when convenient.

- **Pro:** Non-disruptive; user decides when to update.
- **Con:** Requires user action; a child could stay stale indefinitely.

### Option C — Combine A and B
Auto-restart stopped children on rulyeh startup (safe, no work in progress). Surface an indicator in the UI for running children so the user can restart them when ready.

## Recommended Approach

Option C. It handles the common case (stopped children) automatically while respecting in-progress work on running children.
