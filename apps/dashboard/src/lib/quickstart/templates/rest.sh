#!/usr/bin/env bash
set -euo pipefail
{{API_KEY_SH}}
BOXLITE_REST_URL="${BOXLITE_REST_URL:-{{REST_API_URL}}}"
auth=(-H "Authorization: Bearer ${BOXLITE_API_KEY}")
json=(-H "Content-Type: application/json")
name="sdk-quickstart-rest-$(date +%s)-$$"
prefix="$(curl -fsS "${BOXLITE_REST_URL}/v1/me" "${auth[@]}" | jq -r '.path_prefix // empty')"
base="${BOXLITE_REST_URL}/v1${prefix:+/$prefix}"
body="$(jq -n --arg name "$name" '{name:$name,image:"ghcr.io/boxlite-ai/boxlite-agent-base:v0.1.0",network:{inbound:{mode:"enabled"}}}')"
box_id="$(curl -fsS -X POST "$base/boxes" "${auth[@]}" "${json[@]}" -d "$body" | jq -er '.box_id')"
printf 'Created Box: %s\n' "$box_id"
curl -fsS -X POST "$base/boxes/$box_id/start" "${auth[@]}" >/dev/null
{{STEP_EXECUTION}}
exec_id="$(curl -fsS -X POST "$base/boxes/$box_id/exec" "${auth[@]}" "${json[@]}" -d {{EXEC_JSON_SH}} | jq -er '.execution_id')"
{{REST_EXECUTION_RESULT}}
{{STEP_PREVIEW}}
server_code={{SERVER_SH}}
body="$(jq -n --arg server "$server_code" '{command:"python3",args:["-u","-c",$server]}')"
curl -fsS -X POST "$base/boxes/$box_id/exec" "${auth[@]}" "${json[@]}" -d "$body" | jq -e '.execution_id'
url="$(curl -fsS -X POST "$base/boxes/$box_id/network/tunnel?port=3000" "${auth[@]}" "${json[@]}" -d '{}' | jq -er '.uri')"
printf 'Preview URL: %s\n' "$url"
