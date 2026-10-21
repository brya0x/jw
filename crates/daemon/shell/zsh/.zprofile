{
  local _jw_z=$ZDOTDIR
  ZDOTDIR=${JW_USER_ZDOTDIR:-$HOME}
  [[ -r $ZDOTDIR/.zprofile ]] && source $ZDOTDIR/.zprofile
  ZDOTDIR=$_jw_z
}
