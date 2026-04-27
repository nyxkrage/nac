#!/bin/bash
# NAC Terminal Lifecycle Integration Script
# Emits OSC 133 sequences for command lifecycle tracking
# Format: ESC ] <params> ST (ESC \) = \x1b]<params>\x1b\\
# Reference: https://iterm2.com/documentation-escape-sequences.html (Shell Integration)

# Helper function to emit OSC sequences using ST (String Terminator)
# Format: \x1b]133;A\x1b\\ for start, \x1b]133;D;exitcode\x1b\\ for end
_nac_osc() {
    printf '\x1b]%s\x1b\\' "$1"
}

# Emit custom NAC OSC sequences for extended tracking
_nac_osc_custom() {
    printf '\x1b]133;nac;%s\x1b\\' "$1"
}

# Track command start - emitted before command execution
# OSC 133;A - "Prompt" - marks the start of a new command line
_nac_cmd_start() {
    local cmd="$1"
    local cwd="$(pwd)"
    _nac_osc "133;A"
    _nac_osc_custom "cmd_start;$(date +%s%N);$cwd;$cmd"
}

# Track command output start
# OSC 133;C - "Command Output" - marks the start of command output
_nac_output_start() {
    _nac_osc "133;C"
    _nac_osc_custom "output_start"
}

# Track command output end
# OSC 133;D - "Command Finished" - marks the end of command output
# Takes exit code as parameter
_nac_output_end() {
    local exit_code="${1:-0}"
    _nac_osc "133;D;$exit_code"
    _nac_osc_custom "output_end;$exit_code"
}

# Track command end / prompt ready
# OSC 133;B - "Prompt Ready" - marks when prompt is ready for next command
_nac_cmd_end() {
    local exit_code="${1:-0}"
    _nac_osc "133;B"
    _nac_osc_custom "cmd_end;$exit_code;$(date +%s%N)"
}

# Pre-command hook - runs before each command
_nac_preexec() {
    local cmd="$1"
    _nac_cmd_start "$cmd"
    _nac_output_start
}

# Post-command hook - runs after each command
_nac_precmd() {
    local exit_code=$?
    _nac_output_end "$exit_code"
    _nac_cmd_end "$exit_code"
}

# Set up the preexec/precmd hooks using DEBUG trap and PROMPT_COMMAND
# Store the last command for tracking
_nac_last_command=""

# DEBUG trap runs before each command execution
_nac_debug_trap() {
    local cmd="$BASH_COMMAND"
    # Avoid tracking the debug trap itself and internal commands
    if [[ "$cmd" != "_nac_precmd" && "$cmd" != "_nac_debug_trap" && ! "$cmd" =~ ^_nac_ ]]; then
        _nac_last_command="$cmd"
        _nac_preexec "$cmd"
    fi
}

# Install the DEBUG trap
trap '_nac_debug_trap' DEBUG

# PROMPT_COMMAND runs after command completion, before prompt display
# We use an array to avoid clobbering existing PROMPT_COMMANDs
if [[ -z "$PROMPT_COMMAND" ]]; then
    PROMPT_COMMAND="_nac_precmd"
else
    # Check if already installed to avoid duplicates
    if [[ "$PROMPT_COMMAND" != *"_nac_precmd"* ]]; then
        PROMPT_COMMAND="_nac_precmd;${PROMPT_COMMAND}"
    fi
fi

# Set up custom prompt that emits OSC sequences
# The prompt emits OSC 133;A at the start of each prompt line
# Using format: \x1b]133;A\x1b\\ (OSC 133;A with ST terminator)
_nac_prompt_prefix='\[\x1b]133;A\x1b\\\]'
_nac_prompt_suffix='\[\x1b]133;B\x1b\\\]'

# Build the custom prompt with OSC sequences
# Preserve the original prompt if it exists, otherwise use a default
if [[ -n "$PS1" && "$PS1" != *"_nac_"* ]]; then
    # Wrap existing PS1 with our OSC sequences
    export PS1="${_nac_prompt_prefix}${PS1}${_nac_prompt_suffix}"
else
    # Default prompt with OSC sequences
    export PS1="${_nac_prompt_prefix}\u@\h:\w\$ ${_nac_prompt_suffix}"
fi

# Also emit initial "ready" sequence on shell startup
_nac_osc "133;B"
_nac_osc_custom "shell_ready;$$;$(date +%s%N);$(pwd)"

# Export functions so they're available in subshells if needed
export -f _nac_osc 2>/dev/null || true
export -f _nac_osc_custom 2>/dev/null || true
export -f _nac_cmd_start 2>/dev/null || true
export -f _nac_output_start 2>/dev/null || true
export -f _nac_output_end 2>/dev/null || true
export -f _nac_cmd_end 2>/dev/null || true
export -f _nac_preexec 2>/dev/null || true
export -f _nac_precmd 2>/dev/null || true
export -f _nac_debug_trap 2>/dev/null || true
