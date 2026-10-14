# jwn shell integration for bash: the user's ~/.bashrc, then hooks that report each prompt and
# each command to the daemon (OSC 133 and 633). Works on bash 3.2.
[ -r "$HOME/.bashrc" ] && . "$HOME/.bashrc"

__jwn_started=
__jwn_ready=
__jwn_b64() { printf %s "$1" | base64 | tr -d '\n'; }
__jwn_debug() {
  [ -z "$__jwn_ready" ] && return
  [ -n "$__jwn_started" ] && return
  __jwn_started=1
  local line
  line=$(HISTTIMEFORMAT= builtin history 1 | sed 's/^ *[0-9]* *//')
  [ -z "$line" ] && line=$BASH_COMMAND
  printf '\e]633;E;%s\a\e]633;P;Cwd=%s\a\e]133;C\a' "$(__jwn_b64 "$line")" "$(__jwn_b64 "$PWD")"
}
__jwn_precmd() {
  local c=$?
  __jwn_ready=
  if [ -n "$__jwn_started" ]; then printf '\e]133;D;%d\a' "$c"; __jwn_started=; fi
}
__jwn_arm() { __jwn_ready=1; }
trap '__jwn_debug' DEBUG
PROMPT_COMMAND="__jwn_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND};__jwn_arm"
PS1='\[\e]133;A\a\]'"$PS1"'\[\e]133;B\a\]'
