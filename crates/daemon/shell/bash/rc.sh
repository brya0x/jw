# jwn shell integration for bash: the user's ~/.bashrc, then hooks that report each prompt and
# each command to the daemon (OSC 133 and 633). Works on bash 3.2.
[ -r "$HOME/.bashrc" ] && . "$HOME/.bashrc"

__jwn_started=
__jwn_ready=
__jwn_pwd=
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
  # The folder the shell is in, when it changed: what the pane points at follows a `cd`.
  if [ "$PWD" != "$__jwn_pwd" ]; then __jwn_pwd=$PWD; printf '\e]633;P;Pwd=%s\a' "$(__jwn_b64 "$PWD")"; fi
}
__jwn_arm() { __jwn_ready=1; }
trap '__jwn_debug' DEBUG
PROMPT_COMMAND="__jwn_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND};__jwn_arm"
PS1='\[\e]133;A\a\]'"$PS1"'\[\e]133;B\a\]'

# `claude` typed in a pane of the daemon is the binary's own conversation (spec REQ-371); a
# function exists only in the shell the person types in, so a script and `command claude` still
# run the original. Not defined where the person already has an alias or a function of that name.
# `JWN_CONVERSATION=0` turns the swap off (spec REQ-371 and the conversation's settings).
if [ -n "$JWN_CONVERSATION" ] && [ "$JWN_CONVERSATION" != 0 ] && [ -n "$JWN_BIN" ] && [ -n "$JWN_PANE_ID" ] \
  && [ "$(type -t claude)" != function ] && [ "$(type -t claude)" != alias ]; then
  claude() { "$JWN_BIN" claude "$@"; }
fi

# `cd @` goes to the folder of the pane's workspace (spec REQ-645); any other `cd` is as it was. Not
# defined where the person already has a function or an alias named `cd`. A folder that is named `@`
# is `cd ./@`.
if [ -n "$JWN_WORKSPACE_DIR" ] && [ "$(type -t cd)" = builtin ]; then
  cd() {
    if [ $# -eq 1 ] && [ "$1" = @ ]; then
      builtin cd -- "$JWN_WORKSPACE_DIR"
    else
      builtin cd "$@"
    fi
  }
fi
