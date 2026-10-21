{
  local _jwn_z=$ZDOTDIR
  ZDOTDIR=${JWN_USER_ZDOTDIR:-$HOME}
  [[ -r $ZDOTDIR/.zprofile ]] && source $ZDOTDIR/.zprofile
  ZDOTDIR=$_jwn_z
}
