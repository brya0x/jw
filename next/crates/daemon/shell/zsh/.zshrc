# jwn shell integration for zsh: run the user's .zshrc, give ZDOTDIR back to the user, then
# report each prompt and each command to the daemon (OSC 133 and 633).
{
  local _jwn_z=$ZDOTDIR
  ZDOTDIR=${JWN_USER_ZDOTDIR:-$HOME}
  [[ -r $ZDOTDIR/.zshrc ]] && source $ZDOTDIR/.zshrc
  ZDOTDIR=$_jwn_z
}
ZDOTDIR=${JWN_USER_ZDOTDIR:-$HOME}
unset JWN_USER_ZDOTDIR

autoload -Uz add-zsh-hook
typeset -g __jwn_ran=0
__jwn_b64() { printf %s "$1" | base64 | tr -d '\n' }
__jwn_preexec() {
  __jwn_ran=1
  printf '\e]633;E;%s\a\e]633;P;Cwd=%s\a\e]133;C\a' "$(__jwn_b64 "$1")" "$(__jwn_b64 "$PWD")"
}
# First among the precmd hooks, so the status is still the command's.
__jwn_precmd_first() {
  local c=$?
  if (( __jwn_ran )); then printf '\e]133;D;%d\a' "$c"; __jwn_ran=0; fi
}
# Last, so a prompt theme that rebuilds PS1 has already done so.
__jwn_precmd_last() {
  if [[ $PS1 != *$'\e]133;A'* ]]; then
    PS1=$'%{\e]133;A\a%}'"$PS1"$'%{\e]133;B\a%}'
  fi
}
add-zsh-hook preexec __jwn_preexec
precmd_functions=(__jwn_precmd_first $precmd_functions __jwn_precmd_last)

# `claude` typed in a pane of the daemon is the binary's own conversation (spec REQ-371); a
# function exists only in the shell the person types in, so a script and `command claude` still
# run the original. Not defined where the person already has an alias or a function of that name.
if [[ -n "$JWN_CONVERSATION" && -n "$JWN_BIN" && -n "$JWN_PANE_ID" ]] \
  && (( ! $+functions[claude] )) && (( ! $+aliases[claude] )); then
  claude() { "$JWN_BIN" claude "$@"; }
fi
