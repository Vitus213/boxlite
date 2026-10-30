#!/usr/bin/env bash
set -euo pipefail
{{API_KEY_SH}}
export BOXLITE_REST_URL="${BOXLITE_REST_URL:-{{REST_API_URL}}}"
export BOXLITE_REST_PATH_PREFIX="$(curl -fsS "${BOXLITE_REST_URL}/v1/me" \
  -H "Authorization: Bearer ${BOXLITE_API_KEY}" | jq -r '.path_prefix // empty')"
box_name="sdk-quickstart-cli-$(date +%s)-$$"

box_id="$(boxlite create --name "$box_name" --inbound enabled ghcr.io/boxlite-ai/boxlite-agent-base:v0.1.0)"
printf 'Created Box: %s\n' "$box_id"
boxlite start "$box_name"
{{STEP_EXECUTION}}
boxlite exec "$box_name" -- {{SHELL_COMMAND}}
{{STEP_PREVIEW}}
server_code={{SERVER_SH}}
boxlite exec --detach "$box_name" -- python3 -u -c "$server_code"
url="$(boxlite network tunnel "$box_name" 3000)"
if [ -z "$url" ]; then
  printf 'Cloud did not return a preview URL\n' >&2
  exit 1
fi
printf 'Preview URL: %s\n' "$url"
