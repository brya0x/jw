# jw shell integration for zsh: run the user's .zshrc, give ZDOTDIR back to the user, then
# report each prompt and each command to the daemon (OSC 133 and 633).
{
  local _jw_z=$ZDOTDIR
  ZDOTDIR=${JW_USER_ZDOTDIR:-$HOME}
  [[ -r $ZDOTDIR/.zshrc ]] && source $ZDOTDIR/.zshrc
  ZDOTDIR=$_jw_z
}
ZDOTDIR=${JW_USER_ZDOTDIR:-$HOME}
unset JW_USER_ZDOTDIR

autoload -Uz add-zsh-hook
typeset -g __jw_ran=0
typeset -g __jw_pwd=
__jw_b64() { printf %s "$1" | base64 | tr -d '\n' }
__jw_preexec() {
  __jw_ran=1
  printf '\e]633;E;%s\a\e]633;P;Cwd=%s\a\e]133;C\a' "$(__jw_b64 "$1")" "$(__jw_b64 "$PWD")"
}
# First among the precmd hooks, so the status is still the command's.
__jw_precmd_first() {
  local c=$?
  if (( __jw_ran )); then printf '\e]133;D;%d\a' "$c"; __jw_ran=0; fi
  # The folder the shell is in, when it changed: what the pane points at follows a `cd`.
  if [[ $PWD != "$__jw_pwd" ]]; then __jw_pwd=$PWD; printf '\e]633;P;Pwd=%s\a' "$(__jw_b64 "$PWD")"; fi
}
# Last, so a prompt theme that rebuilds PS1 has already done so.
__jw_precmd_last() {
  if [[ $PS1 != *$'\e]133;A'* ]]; then
    PS1=$'%{\e]133;A\a%}'"$PS1"$'%{\e]133;B\a%}'
  fi
}
add-zsh-hook preexec __jw_preexec
precmd_functions=(__jw_precmd_first $precmd_functions __jw_precmd_last)

# `claude` typed in a pane of the daemon is the binary's own conversation (spec REQ-371); a
# function exists only in the shell the person types in, so a script and `command claude` still
# run the original. Not defined where the person already has an alias or a function of that name.
# `JW_CONVERSATION=0` turns the swap off (spec REQ-371 and the conversation's settings).
if [[ -n "$JW_CONVERSATION" && "$JW_CONVERSATION" != 0 && -n "$JW_BIN" && -n "$JW_PANE_ID" ]] \
  && (( ! $+functions[claude] )) && (( ! $+aliases[claude] )); then
  claude() { "$JW_BIN" claude "$@"; }
fi

# `cd @` goes to the folder of the pane's workspace (spec REQ-645); any other `cd` is as it was. Not
# defined where the person already has a function or an alias named `cd`. A folder that is named `@`
# is `cd ./@`.
if [[ -n "$JW_WORKSPACE_DIR" ]] && (( ! $+functions[cd] )) && (( ! $+aliases[cd] )); then
  cd() {
    if (( $# == 1 )) && [[ $1 == @ ]]; then
      builtin cd -- "$JW_WORKSPACE_DIR"
    else
      builtin cd "$@"
    fi
  }
fi
