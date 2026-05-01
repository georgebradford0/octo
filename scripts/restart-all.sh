#!/usr/bin/env sh
# Rollout-restart rulyeh and all managed child deployments.
# Usage: ./scripts/restart-all.sh
set -e

kubectl rollout restart deployment/rulyeh -n claudulhu
kubectl rollout restart deployment -l claudulhu.managed=1 -n claudulhu
echo "Rollout restart triggered for rulyeh and all managed children."
