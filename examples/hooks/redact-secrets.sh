#!/usr/bin/env bash
# Hook: redact-secrets
# Events: ToolResultPersist
# Strips common secret patterns from tool results before they are persisted.

set -euo pipefail

payload=$(cat)
event=$(echo "$payload" | jq -r '.event')

if [ "$event" != "ToolResultPersist" ]; then
    exit 0
fi

# Replace the persisted `result` value with a redacted copy.
redacted_result=$(echo "$payload" | jq -c '
  .result |= (.. | strings |= (
    gsub("sk-[A-Za-z0-9]{20,}"; "[REDACTED]") |
    gsub("ghp_[A-Za-z0-9]{36,}"; "[REDACTED]") |
    gsub("xoxb-[A-Za-z0-9-]+"; "[REDACTED]")
  )) |
  .result
')

echo "{\"action\":\"modify\",\"data\":$redacted_result}"
