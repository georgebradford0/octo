#!/usr/bin/env bash
set -euo pipefail

ssh -i /Users/georgebalch/Documents/lenovo-ideapad.pem \
    ubuntu@ec2-35-88-113-219.us-west-2.compute.amazonaws.com \
    "sudo kubectl rollout restart deployment/rulyeh -n claudulhu && sudo kubectl rollout status deployment/rulyeh -n claudulhu --timeout=120s"
