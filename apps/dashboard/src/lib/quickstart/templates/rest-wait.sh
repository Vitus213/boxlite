deadline=$((SECONDS + 900))
while :; do
  status="$(curl -fsS --connect-timeout 5 --max-time 15 \
    "$base/boxes/$box_id/executions/$exec_id" "${auth[@]}")"
  case "$(printf '%s' "$status" | jq -r '.status')" in
    completed|killed|timed_out)
      printf '%s\n' "$status" | jq
      exit_code="$(printf '%s' "$status" | jq -r '.exit_code // 1')"
      if [ "$exit_code" != 0 ]; then exit 1; fi
      break
      ;;
  esac
  if [ "$SECONDS" -ge "$deadline" ]; then
    printf 'Execution did not finish within 15 minutes.\n' >&2
    exit 124
  fi
  sleep 1
done
