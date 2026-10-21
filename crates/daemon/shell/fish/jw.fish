# jw shell integration for fish: report each prompt and each command to the daemon
# (OSC 133 and 633).
set -g __jw_ran 0
set -g __jw_pwd ''
function __jw_b64
    printf %s $argv[1] | base64 | tr -d '\n'
end
function __jw_preexec --on-event fish_preexec
    set -g __jw_ran 1
    printf '\e]633;E;%s\a\e]633;P;Cwd=%s\a\e]133;C\a' (__jw_b64 "$argv[1]") (__jw_b64 "$PWD")
end
functions -c fish_prompt __jw_orig_prompt
function fish_prompt
    set -l c $status
    if test $__jw_ran = 1
        printf '\e]133;D;%d\a' $c
        set -g __jw_ran 0
    end
    # The folder the shell is in, when it changed: what the pane points at follows a `cd`.
    if test "$PWD" != "$__jw_pwd"
        set -g __jw_pwd $PWD
        printf '\e]633;P;Pwd=%s\a' (__jw_b64 "$PWD")
    end
    printf '\e]133;A\a'
    __jw_orig_prompt
    printf '\e]133;B\a'
end

# `claude` typed in a pane of the daemon is the binary's own conversation (spec REQ-371); a
# function exists only in the shell the person types in, so a script and `command claude` still
# run the original. fish runs this before the person's config, so a function of theirs wins.
# `JW_CONVERSATION=0` turns the swap off.
if test -n "$JW_CONVERSATION"; and test "$JW_CONVERSATION" != 0; and set -q JW_BIN; and set -q JW_PANE_ID
    function claude --description 'the conversation of this pane'
        $JW_BIN claude $argv
    end
end

# `cd @` goes to the folder of the pane's workspace (spec REQ-645); any other `cd` is as it was. fish
# runs this before the person's config, so a function of theirs named `cd` wins.
if set -q JW_WORKSPACE_DIR; and test -n "$JW_WORKSPACE_DIR"
    functions -c cd __jw_orig_cd
    function cd --wraps cd --description 'cd, and cd @ to the workspace folder'
        if test (count $argv) -eq 1; and test "$argv[1]" = @
            __jw_orig_cd $JW_WORKSPACE_DIR
        else
            __jw_orig_cd $argv
        end
    end
end
