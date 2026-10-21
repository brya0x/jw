# jw shell integration: run the user's own file, keeping ZDOTDIR on jw's directory so the
# files that follow are jw's too.
{
  local _jw_z=$ZDOTDIR
  ZDOTDIR=${JW_USER_ZDOTDIR:-$HOME}
  [[ -r $ZDOTDIR/.zshenv ]] && source $ZDOTDIR/.zshenv
  ZDOTDIR=$_jw_z
}
