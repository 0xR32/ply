#!/bin/sh
# Stand-in for Codex in plyd's tests; the protocol is documented in crates/daemon/tests/common/fake.rs.
args="$*"
hook=
resume=
prompt=
while [ $# -gt 0 ]; do
  case "$1" in
    -c)
      case "$2" in notify=*) hook=$(printf '%s' "$2" | sed -n 's/^notify=\["\(.*\)","codex"\]$/\1/p') ;; esac
      shift 2 ;;
    resume) resume=$2; shift 2 ;;
    --) shift; prompt=$*; break ;;
    *) prompt=$1; shift ;;
  esac
done
cwd=$(pwd -P)
printf 'launch pane=%s cwd=%s argv=%s\n' "$PLY_PANE_ID" "$cwd" "$args" >> "$HOME/fake-codex.log"
home=${CODEX_HOME:-$HOME/.codex}
thread=${resume:-00000000-0000-7000-8000-$(printf '%012d' "$PLY_PANE_ID")}
rollout=
if [ -n "$resume" ]; then
  rollout=$(find "$home/sessions" -name "rollout-*-$resume.jsonl" 2>/dev/null | head -n 1)
fi

record() {
  printf '%s\n' "$1" >> "$rollout"
}

session() {
  dir="$home/sessions/$(date +%Y/%m/%d)"
  mkdir -p "$dir"
  rollout="$dir/rollout-$(date +%Y-%m-%dT%H-%M-%S)-$thread.jsonl"
  record "{\"timestamp\":\"$(date -u +%Y-%m-%dT%H:%M:%S.000Z)\",\"ordinal\":0,\"type\":\"session_meta\",\"payload\":{\"id\":\"$thread\",\"session_id\":\"$thread\",\"cwd\":\"$cwd\",\"cli_version\":\"0.156.1\"}}"
}

notify() {
  "$hook" codex "{\"type\":\"agent-turn-complete\",\"thread-id\":\"$1\",\"turn-id\":\"00000000-0000-7000-8000-00000000ffff\",\"cwd\":\"$cwd\",\"last-assistant-message\":\"done\"}"
}

fifo="$HOME/fake-$PLY_PANE_ID.cmd"
rm -f "$fifo"
mkfifo "$fifo" || exit 71
exec 3<>"$fifo"
printf 'fake codex %s in %s\r\n' "$thread" "$cwd"
while IFS= read -r line <&3; do
  cmd=${line%% *}
  rest=${line#* }
  [ "$rest" = "$line" ] && rest=
  case "$cmd" in
    session) session ;;
    record) record "$rest" ;;
    notify) notify "${rest:-$thread}" ;;
    osc9) printf '\033]9;%s\007' "$rest" ;;
    out) printf '%s\r\n' "$rest" ;;
    exit) rm -f "$fifo"; exit "$rest" ;;
  esac
done
