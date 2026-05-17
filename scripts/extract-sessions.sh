#!/usr/bin/env bash
#
# Extract AI coding session logs and save them as readable Markdown into ../docs/.
#
# Sessions extracted:
#   - codex-implementation       — Codex CLI session that vibe-coded the implementation
#   - codex-adversarial-review   — adversarial review run via Codex
#   - claude-adversarial-review  — adversarial review by Claude Code
#
# Re-run any time to refresh the extracted transcripts.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DOCS_DIR="$(cd "${SCRIPT_DIR}/../docs" && pwd)"

CODEX_IMPL_SESSION="${HOME}/.codex/sessions/2026/05/17/rollout-2026-05-17T14-16-33-019e35dd-df14-7320-879c-96e2662226d6.jsonl"
CODEX_REVIEW_SESSION="${HOME}/.claude/projects/-Users-flo-Developer-github-async-request-queue-challenge/c8e2ea40-a22f-46f5-90df-e85eed8f981d.jsonl"
CLAUDE_REVIEW_SESSION="${HOME}/.claude/projects/-Users-flo-Developer-github-async-request-queue-challenge/de887e8d-1e16-41b3-9b97-e1d6f19a5d6d.jsonl"

CODEX_IMPL_OUT="${DOCS_DIR}/SESSION_CODEX_IMPLEMENTATION.md"
CODEX_REVIEW_OUT="${DOCS_DIR}/SESSION_CODEX_ADVERSARIAL_REVIEW.md"
CLAUDE_REVIEW_OUT="${DOCS_DIR}/SESSION_CLAUDE_ADVERSARIAL_REVIEW.md"

command -v jq >/dev/null || { echo "jq is required" >&2; exit 1; }

extract_codex() {
    local src="$1" out="$2" title="$3"
    [[ -f "$src" ]] || { echo "Codex session not found: $src" >&2; return 1; }

    {
        printf '# %s\n\n' "$title"
        printf '_Source: `%s`_\n\n' "$src"
        printf -- '---\n\n'

        jq -r '
            select(.type == "response_item" and .payload.type == "message")
            | .payload as $m
            | "## " + ($m.role | ascii_upcase) + "\n\n"
              + ([$m.content[]?
                    | (.text // .input_text // .output_text // empty)
                 ] | join("\n\n"))
              + "\n\n---\n"
        ' "$src"
    } > "$out"

    echo "Wrote $out"
}

extract_claude() {
    local src="$1" out="$2" title="$3"
    [[ -f "$src" ]] || { echo "Claude session not found: $src" >&2; return 1; }

    {
        printf '# %s\n\n' "$title"
        printf '_Source: `%s`_\n\n' "$src"
        printf -- '---\n\n'

        jq -r '
            select(.type == "user" or .type == "assistant")
            | .message as $m
            | ($m.role | ascii_upcase) as $role
            | if ($m.content | type) == "string" then
                "## " + $role + "\n\n" + $m.content + "\n\n---\n"
              else
                "## " + $role + "\n\n"
                + ([$m.content[]?
                     | if .type == "text" then .text
                       elif .type == "tool_use" then
                         "**[tool: " + .name + "]**\n\n```json\n"
                         + (.input | tostring) + "\n```"
                       elif .type == "tool_result" then
                         "**[tool result]**\n\n```\n"
                         + (.content | tostring) + "\n```"
                       else empty end
                   ] | join("\n\n"))
                + "\n\n---\n"
              end
        ' "$src"
    } > "$out"

    echo "Wrote $out"
}

extract_codex  "$CODEX_IMPL_SESSION"    "$CODEX_IMPL_OUT"    "Codex Implementation"
extract_claude "$CODEX_REVIEW_SESSION"  "$CODEX_REVIEW_OUT"  "Codex Adversarial Review"
extract_claude "$CLAUDE_REVIEW_SESSION" "$CLAUDE_REVIEW_OUT" "Claude Adversarial Review"
