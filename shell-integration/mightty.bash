# mightty shell integration for interactive Bash sessions.

[[ -n "${BASH_VERSION-}" && "$-" == *i* ]] || return 0
[[ -z "${__MIGHTTY_SHELL_INTEGRATION_LOADED-}" ]] || return 0
__MIGHTTY_SHELL_INTEGRATION_LOADED=1

__mightty_encode_path() {
    local input="$1" output="" character encoded index LC_ALL=C
    for ((index = 0; index < ${#input}; index++)); do
        character="${input:index:1}"
        case "$character" in
            [a-zA-Z0-9/._~-]) output+="$character" ;;
            *)
                printf -v encoded '%%%02X' "'$character"
                output+="$encoded"
                ;;
        esac
    done
    printf '%s' "$output"
}

__mightty_precmd() {
    local status="$?"
    printf '\e]133;D;%s\a' "$status"
    printf '\e]7;file://localhost%s\a' "$(__mightty_encode_path "$PWD")"
    printf '\e]133;A\a'
    [[ "$PS1" == *'\e]133;B\a'* ]] || PS1+='\[\e]133;B\a\]'
}

if declare -p PROMPT_COMMAND 2>/dev/null | grep -q 'declare -a'; then
    PROMPT_COMMAND=(__mightty_precmd "${PROMPT_COMMAND[@]}")
else
    PROMPT_COMMAND="__mightty_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
fi

PS0=$'\e]133;C\a'"${PS0-}"
