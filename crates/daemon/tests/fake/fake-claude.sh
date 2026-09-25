#!/bin/sh
# Stand-in for Claude Code in plyd's tests; the protocol is documented in crates/daemon/tests/common/fake.rs.
args="$*"
settings=
worktree=
resume=
prompt=
while [ $# -gt 0 ]; do
  case "$1" in
    --settings) settings=$2; shift 2 ;;
    --worktree) worktree=$2; shift 2 ;;
    --resume) resume=$2; shift 2 ;;
    --) shift; prompt=$*; break ;;
    *) prompt=$1; shift ;;
  esac
done
if [ -n "$worktree" ]; then
  mkdir -p ".claude/worktrees/$worktree" && cd ".claude/worktrees/$worktree" || exit 70
fi
cwd=$(pwd -P)
printf 'launch pane=%s cwd=%s argv=%s\n' "$PLY_PANE_ID" "$cwd" "$args" >> "$HOME/fake-claude.log"
sid=${resume:-00000000-0000-4000-8000-$(printf '%012d' "$PLY_PANE_ID")}
source=startup
[ -n "$resume" ] && source=resume

hook() {
  cmd=$(sed -n "s/^ *\"command\": \"\(.* claude $1\)\",\{0,1\}\$/\1/p" "$settings")
  printf '%s' "$2" | sh -c "$cmd"
}

fifo="$HOME/fake-$PLY_PANE_ID.cmd"
rm -f "$fifo"
mkfifo "$fifo" || exit 71
exec 3<>"$fifo"
start() {
  hook SessionStart "{\"session_id\":\"$sid\",\"cwd\":\"$cwd\",\"hook_event_name\":\"SessionStart\",\"source\":\"$source\",\"model\":\"claude-example-model\"}"
  if [ -n "$prompt" ]; then
    hook UserPromptSubmit "{\"session_id\":\"$sid\",\"cwd\":\"$cwd\",\"hook_event_name\":\"UserPromptSubmit\",\"prompt\":\"$prompt\"}"
  fi
}

printf 'fake claude %s in %s\r\n' "$sid" "$cwd"
if [ "$prompt" = "wait-for-start" ]; then
  prompt=
else
  start
fi
while IFS= read -r line <&3; do
  cmd=${line%% *}
  rest=${line#* }
  case "$cmd" in
    start) start ;;
    hook) hook "${rest%% *}" "${rest#* }" ;;
    out) printf '%s\r\n' "$rest" ;;
    spin) i=0; while [ "$i" -lt "$rest" ]; do printf '.'; sleep 0.1; i=$((i + 1)); done; printf '\r\n' ;;
    keys) stty raw -echo; : > "$HOME/fake-keys.ready"; dd bs=1 count="$rest" 2>/dev/null | od -An -tx1 | tr -d ' \n' >> "$HOME/fake-keys.log"; stty sane; rm -f "$HOME/fake-keys.ready" ;;
    exit) rm -f "$fifo"; exit "$rest" ;;
  esac
done
