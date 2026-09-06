# Print an optspec for argparse to handle cmd's options that are independent of any subcommand.
function __fish_qdrop_global_optspecs
    string join \n v/verbose q/quiet h/help V/version
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
complete -c qdrop -n "__fish_qdrop_needs_command" -s q -l quiet -d 'Suppress progress/status output (errors still print)'
complete -c qdrop -n "__fish_qdrop_needs_command" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_needs_command" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "pair" -d 'Pair with another device, or manage existing pairings'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "peers" -d 'List paired peers and their status'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "send" -d 'Send one or more files to a peer (use `-` for stdin)'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "recv" -d 'Wait for an incoming file; print its path or stream it to stdout'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "paste" -d 'Print a peer\'s shared clipboard text to stdout'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "copy" -d 'Read stdin and set it as the shared clipboard'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "open" -d 'Open a URL on a peer'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "clip" -d 'Control clipboard synchronization'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "auth" -d 'Pre-authorize a paired peer to send here unattended'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "status" -d 'Show live daemon status (peers, clipboard)'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "doctor" -d 'Diagnose why sync isn\'t working'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "daemon" -d 'Run the qdrop daemon in the foreground'
complete -c qdrop -n "__fish_qdrop_needs_command" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c qdrop -n "__fish_qdrop_using_subcommand pair" -l remove -d 'Remove an existing pairing by name' -r
complete -c qdrop -n "__fish_qdrop_using_subcommand pair" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_using_subcommand pair" -s q -l quiet -d 'Suppress progress/status output (errors still print)'
complete -c qdrop -n "__fish_qdrop_using_subcommand pair" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_using_subcommand pair" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_using_subcommand peers" -l json -d 'Emit JSON instead of a table'
complete -c qdrop -n "__fish_qdrop_using_subcommand peers" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_using_subcommand peers" -s q -l quiet -d 'Suppress progress/status output (errors still print)'
complete -c qdrop -n "__fish_qdrop_using_subcommand peers" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_using_subcommand peers" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_using_subcommand send" -l to -d 'Target peer name. Defaults to all connected peers' -r
complete -c qdrop -n "__fish_qdrop_using_subcommand send" -l name -d 'Filename to use for stdin (`-`). Default `stdin-<timestamp>.bin`' -r
complete -c qdrop -n "__fish_qdrop_using_subcommand send" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_using_subcommand send" -s q -l quiet -d 'Suppress progress/status output (errors still print)'
complete -c qdrop -n "__fish_qdrop_using_subcommand send" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_using_subcommand send" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_using_subcommand recv" -l timeout -d 'Give up after this many seconds (default 300)' -r
complete -c qdrop -n "__fish_qdrop_using_subcommand recv" -l stdout -d 'Write the file\'s contents to stdout instead of leaving it on disk'
complete -c qdrop -n "__fish_qdrop_using_subcommand recv" -l keep -d 'With `--stdout`, keep the file in the download folder too'
complete -c qdrop -n "__fish_qdrop_using_subcommand recv" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_using_subcommand recv" -s q -l quiet -d 'Suppress progress/status output (errors still print)'
complete -c qdrop -n "__fish_qdrop_using_subcommand recv" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_using_subcommand recv" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_using_subcommand paste" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_using_subcommand paste" -s q -l quiet -d 'Suppress progress/status output (errors still print)'
complete -c qdrop -n "__fish_qdrop_using_subcommand paste" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_using_subcommand paste" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_using_subcommand copy" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_using_subcommand copy" -s q -l quiet -d 'Suppress progress/status output (errors still print)'
complete -c qdrop -n "__fish_qdrop_using_subcommand copy" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_using_subcommand copy" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_using_subcommand open" -l to -d 'Target peer name' -r
complete -c qdrop -n "__fish_qdrop_using_subcommand open" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_using_subcommand open" -s q -l quiet -d 'Suppress progress/status output (errors still print)'
complete -c qdrop -n "__fish_qdrop_using_subcommand open" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_using_subcommand open" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_using_subcommand clip" -l pause -d 'Pause clipboard sync'
complete -c qdrop -n "__fish_qdrop_using_subcommand clip" -l resume -d 'Resume clipboard sync'
complete -c qdrop -n "__fish_qdrop_using_subcommand clip" -l toggle -d 'Flip between paused and active'
complete -c qdrop -n "__fish_qdrop_using_subcommand clip" -l status -d 'Show clipboard sync status (the default)'
complete -c qdrop -n "__fish_qdrop_using_subcommand clip" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_using_subcommand clip" -s q -l quiet -d 'Suppress progress/status output (errors still print)'
complete -c qdrop -n "__fish_qdrop_using_subcommand clip" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_using_subcommand clip" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_using_subcommand auth" -l remove -d 'Revoke authorization for this peer' -r
complete -c qdrop -n "__fish_qdrop_using_subcommand auth" -l mutual -d 'Also ask the peer to authorize this machine back'
complete -c qdrop -n "__fish_qdrop_using_subcommand auth" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_using_subcommand auth" -s q -l quiet -d 'Suppress progress/status output (errors still print)'
complete -c qdrop -n "__fish_qdrop_using_subcommand auth" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_using_subcommand auth" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_using_subcommand status" -l json -d 'Emit the raw JSON from the daemon'
complete -c qdrop -n "__fish_qdrop_using_subcommand status" -l waybar -d 'Emit a single line of waybar JSON (`text` / `tooltip` / `class`)'
complete -c qdrop -n "__fish_qdrop_using_subcommand status" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_using_subcommand status" -s q -l quiet -d 'Suppress progress/status output (errors still print)'
complete -c qdrop -n "__fish_qdrop_using_subcommand status" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_using_subcommand status" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_using_subcommand doctor" -l json -d 'Emit the checks as JSON'
complete -c qdrop -n "__fish_qdrop_using_subcommand doctor" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_using_subcommand doctor" -s q -l quiet -d 'Suppress progress/status output (errors still print)'
complete -c qdrop -n "__fish_qdrop_using_subcommand doctor" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_using_subcommand doctor" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_using_subcommand daemon" -l port -d 'Override the listen port from config' -r
complete -c qdrop -n "__fish_qdrop_using_subcommand daemon" -s v -l verbose -d 'Increase log verbosity (debug). Overridden by `RUST_LOG`'
complete -c qdrop -n "__fish_qdrop_using_subcommand daemon" -s q -l quiet -d 'Suppress progress/status output (errors still print)'
complete -c qdrop -n "__fish_qdrop_using_subcommand daemon" -s h -l help -d 'Print help'
complete -c qdrop -n "__fish_qdrop_using_subcommand daemon" -s V -l version -d 'Print version'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send recv paste copy open clip auth status doctor daemon help" -f -a "pair" -d 'Pair with another device, or manage existing pairings'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send recv paste copy open clip auth status doctor daemon help" -f -a "peers" -d 'List paired peers and their status'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send recv paste copy open clip auth status doctor daemon help" -f -a "send" -d 'Send one or more files to a peer (use `-` for stdin)'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send recv paste copy open clip auth status doctor daemon help" -f -a "recv" -d 'Wait for an incoming file; print its path or stream it to stdout'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send recv paste copy open clip auth status doctor daemon help" -f -a "paste" -d 'Print a peer\'s shared clipboard text to stdout'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send recv paste copy open clip auth status doctor daemon help" -f -a "copy" -d 'Read stdin and set it as the shared clipboard'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send recv paste copy open clip auth status doctor daemon help" -f -a "open" -d 'Open a URL on a peer'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send recv paste copy open clip auth status doctor daemon help" -f -a "clip" -d 'Control clipboard synchronization'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send recv paste copy open clip auth status doctor daemon help" -f -a "auth" -d 'Pre-authorize a paired peer to send here unattended'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send recv paste copy open clip auth status doctor daemon help" -f -a "status" -d 'Show live daemon status (peers, clipboard)'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send recv paste copy open clip auth status doctor daemon help" -f -a "doctor" -d 'Diagnose why sync isn\'t working'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send recv paste copy open clip auth status doctor daemon help" -f -a "daemon" -d 'Run the qdrop daemon in the foreground'
complete -c qdrop -n "__fish_qdrop_using_subcommand help; and not __fish_seen_subcommand_from pair peers send recv paste copy open clip auth status doctor daemon help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
