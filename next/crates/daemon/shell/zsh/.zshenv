# jwn shell integration: run the user's own file, keeping ZDOTDIR on jwn's directory so the
# files that follow are jwn's too.
{
  local _jwn_z=$ZDOTDIR
  ZDOTDIR=${JWN_USER_ZDOTDIR:-$HOME}
  [[ -r $ZDOTDIR/.zshenv ]] && source $ZDOTDIR/.zshenv
  ZDOTDIR=$_jwn_z
}
