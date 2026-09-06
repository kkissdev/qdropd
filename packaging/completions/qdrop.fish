# Print an optspec for argparse to handle cmd's options that are independent of any subcommand.
function __fish_qdrop_global_optspecs
    string join \n v/verbose h/help V/version
end

function __fish_qdrop_needs_command
    # Figure out if the current invocation already has a command.
    set -l cmd (commandline -opc)
    set -e cmd[1]
    argparse -s (__fish_qdrop_global_optspecs) -- $cmd 2>/dev/null
    or return
    if set -q argv[1]
        # Also print the command, so this can be used to figure out what it is.
        echo $argv[1]
        return 1
    end
    return 0
end

function __fish_qdrop_using_subcommand
    set -l cmd (__fish_qdrop_needs_command)
    test -z "$cmd"
    and return 1
    contains -- $cmd[1] $argv
end

complete -c qdrop -n "__fish_qdrop_needs_command" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_needs_command" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_needs_command" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "pair" -d 'Pair with another device, or manage existing pairings'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "peers" -d 'List paired peers and their status'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "send" -d 'Send one or more files to a peer'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "open" -d 'Open a URL on a peer'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "clip" -d 'Control clipboard synchronization'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "status" -d 'Show live daemon status (peers, clipboard)'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "daemon" -d 'Run the qdrop daemon in the foreground'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c qdrop -n "__fish_qdrop_using_subcommand pair" -l remove -d 'Remove an existing pairing by name' -r
complete -c qdrop -n "__fish_qdrop_using_subcommand pair" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_using_subcommand pair" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_using_subcommand pair" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_using_subcommand peers" -l json -d 'Emit JSON instead of a table'
complete -c qdrop -n "__fish_qdrop_using_subcommand peers" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_using_subcommand peers" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_using_subcommand peers" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_using_subcommand send" -l to -d 'Target peer name. Defaults to all paired peers (or the sole peer)' -r
complete -c qdrop -n "__fish_qdrop_using_subcommand send" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_using_subcommand send" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_using_subcommand send" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_using_subcommand open" -l to -d 'Target peer name' -r
complete -c qdrop -n "__fish_qdrop_using_subcommand open" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_using_subcommand open" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_using_subcommand open" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_using_subcommand clip" -l pause -d 'Pause clipboard sync'
complete -c qdrop -n "__fish_qdrop_using_subcommand clip" -l resume -d 'Resume clipboard sync'
complete -c qdrop -n "__fish_qdrop_using_subcommand clip" -l toggle -d 'Flip between paused and active'
complete -c qdrop -n "__fish_qdrop_using_subcommand clip" -l status -d 'Show clipboard sync status (the default)'
complete -c qdrop -n "__fish_qdrop_using_subcommand clip" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_using_subcommand clip" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_using_subcommand clip" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_using_subcommand status" -l json -d 'Emit the raw JSON from the daemon'
complete -c qdrop -n "__fish_qdrop_using_subcommand status" -l waybar -d 'Emit a single line of waybar JSON (`text` / `tooltip` / `class`)'
complete -c qdrop -n "__fish_qdrop_using_subcommand status" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_using_subcommand status" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_using_subcommand status" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_using_subcommand daemon" -l port -d 'Override the listen port from config' -r
complete -c qdrop -n "__fish_qdrop_using_subcommand daemon" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_using_subcommand daemon" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_using_subcommand daemon" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send open clip status daemon help" -f -a "pair" -d 'Pair with another device, or manage existing pairings'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send open clip status daemon help" -f -a "peers" -d 'List paired peers and their status'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send open clip status daemon help" -f -a "send" -d 'Send one or more files to a peer'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send open clip status daemon help" -f -a "open" -d 'Open a URL on a peer'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send open clip status daemon help" -f -a "clip" -d 'Control clipboard synchronization'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send open clip status daemon help" -f -a "status" -d 'Show live daemon status (peers, clipboard)'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send open clip status daemon help" -f -a "daemon" -d 'Run the qdrop daemon in the foreground'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send open clip status daemon help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
