# jw shell integration for bash: the user's ~/.bashrc, then hooks that report each prompt and
# each command to the daemon (OSC 133 and 633). Works on bash 3.2.
[ -r "$HOME/.bashrc" ] && . "$HOME/.bashrc"

__jw_started=
__jw_ready=
__jw_pwd=
__jw_b64() { printf %s "$1" | base64 | tr -d '\n'; }
__jw_debug() {
  [ -z "$__jw_ready" ] && return
  [ -n "$__jw_started" ] && return
  __jw_started=1
  local line
  line=$(HISTTIMEFORMAT= builtin history 1 | sed 's/^ *[0-9]* *//')
  [ -z "$line" ] && line=$BASH_COMMAND
  printf '\e]633;E;%s\a\e]633;P;Cwd=%s\a\e]133;C\a' "$(__jw_b64 "$line")" "$(__jw_b64 "$PWD")"
}
__jw_precmd() {
  local c=$?
  __jw_ready=
  if [ -n "$__jw_started" ]; then printf '\e]133;D;%d\a' "$c"; __jw_started=; fi
  # The folder the shell is in, when it changed: what the pane points at follows a `cd`.
  if [ "$PWD" != "$__jw_pwd" ]; then __jw_pwd=$PWD; printf '\e]633;P;Pwd=%s\a' "$(__jw_b64 "$PWD")"; fi
}
__jw_arm() { __jw_ready=1; }
trap '__jw_debug' DEBUG
PROMPT_COMMAND="__jw_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND};__jw_arm"
PS1='\[\e]133;A\a\]'"$PS1"'\[\e]133;B\a\]'

# `claude` typed in a pane of the daemon is the binary's own conversation (spec REQ-371); a
# function exists only in the shell the person types in, so a script and `command claude` still
# run the original. Not defined where the person already has an alias or a function of that name.
# `JW_CONVERSATION=0` turns the swap off (spec REQ-371 and the conversation's settings).
if [ -n "$JW_CONVERSATION" ] && [ "$JW_CONVERSATION" != 0 ] && [ -n "$JW_BIN" ] && [ -n "$JW_PANE_ID" ] \
  && [ "$(type -t claude)" != function ] && [ "$(type -t claude)" != alias ]; then
  claude() { "$JW_BIN" claude "$@"; }
fi

# `cd @` goes to the folder of the pane's workspace (spec REQ-645); any other `cd` is as it was. Not
# defined where the person already has a function or an alias named `cd`. A folder that is named `@`
# is `cd ./@`.
if [ -n "$JW_WORKSPACE_DIR" ] && [ "$(type -t cd)" = builtin ]; then
  cd() {
    if [ $# -eq 1 ] && [ "$1" = @ ]; then
      builtin cd -- "$JW_WORKSPACE_DIR"
    else
      builtin cd "$@"
    fi
  }
fi
