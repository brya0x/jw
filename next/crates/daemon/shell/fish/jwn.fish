# jwn shell integration for fish: report each prompt and each command to the daemon
# (OSC 133 and 633).
set -g __jwn_ran 0
function __jwn_b64
    printf %s $argv[1] | base64 | tr -d '\n'
end
function __jwn_preexec --on-event fish_preexec
    set -g __jwn_ran 1
    printf '\e]633;E;%s\a\e]633;P;Cwd=%s\a\e]133;C\a' (__jwn_b64 "$argv[1]") (__jwn_b64 "$PWD")
end
functions -c fish_prompt __jwn_orig_prompt
function fish_prompt
    set -l c $status
    if test $__jwn_ran = 1
        printf '\e]133;D;%d\a' $c
        set -g __jwn_ran 0
    end
    printf '\e]133;A\a'
    __jwn_orig_prompt
    printf '\e]133;B\a'
end

# `claude` typed in a pane of the daemon is the binary's own conversation (spec REQ-371); a
# function exists only in the shell the person types in, so a script and `command claude` still
# run the original. fish runs this before the person's config, so a function of theirs wins.
if set -q JWN_CONVERSATION; and set -q JWN_BIN; and set -q JWN_PANE_ID
    function claude --description 'the conversation of this pane'
        $JWN_BIN claude $argv
    end
end
