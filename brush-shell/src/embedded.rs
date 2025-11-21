use crate::args::{CommandLineArgs, InputBackend};
use crate::brushctl;
use crate::error_formatter;
use crate::events;
use crate::productinfo;
use crate::shell_factory;
use brush_interactive::InteractiveShell;
use std::{path::Path, sync::Arc};
use tokio::sync::Mutex;

#[allow(unused_imports, reason = "only used in some configs")]
use std::io::IsTerminal;

// Duplicate of logic from entry.rs to avoid modifying it.
impl CommandLineArgs {
    fn try_parse_from_embedded(itr: impl IntoIterator<Item = String>) -> Result<Self, clap::Error> {
        let (mut this, script_args) = brush_core::builtins::try_parse_known::<Self>(itr)?;

        // if we have `--` and unparsed raw args than
        if let Some(args) = script_args {
            this.script_args.extend(args);
        }

        Ok(this)
    }
}

/// Run the brush shell with provided arguments and extra builtins. Returns the exit code.
pub fn run_custom(
    args: Vec<String>,
    extra_builtins: Vec<(String, brush_core::builtins::Registration)>,
) -> i32 {
    //
    // Install panic handlers to clean up on panic.
    //
    install_panic_handlers();

    let mut args = args;

    // Work around clap's limitations handling +O options.
    for arg in &mut args {
        if arg.starts_with("+O") {
            arg.insert_str(0, "--");
        }
    }

    let parsed_args = match CommandLineArgs::try_parse_from_embedded(args.iter().cloned()) {
        Ok(parsed_args) => parsed_args,
        Err(e) => {
            let _ = e.print();

            // Check for whether this is something we'd truly consider fatal. clap returns
            // errors for `--help`, `--version`, etc.
            let exit_code = match e.kind() {
                clap::error::ErrorKind::DisplayVersion => 0,
                clap::error::ErrorKind::DisplayHelp => 0,
                _ => 1,
            };

            return exit_code;
        }
    };

    //
    // Run.
    //
    #[cfg(any(unix, windows))]
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    #[cfg(not(any(unix, windows)))]
    let mut builder = tokio::runtime::Builder::new_current_thread();

    let result = builder
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_async(args, parsed_args, extra_builtins));

    match result {
        Ok(code) => i32::from(code),
        Err(err) => {
            tracing::error!("error: {err:#}");
            1
        }
    }
}

/// Installs panic handlers to report our panic and cleanly exit on panic.
fn install_panic_handlers() {
    //
    // Set up panic handler. On release builds, it will capture panic details to a
    // temporary .toml file and report a human-readable message to the screen.
    //
    human_panic::setup_panic!(
        human_panic::Metadata::new(productinfo::PRODUCT_NAME, productinfo::PRODUCT_VERSION)
            .homepage(env!("CARGO_PKG_HOMEPAGE"))
            .support("please post a GitHub issue at https://github.com/reubeno/brush/issues/new")
    );

    //
    // If stdout is connected to a terminal, then register a new panic handler that
    // resets the terminal and then invokes the previously registered handler. In
    // dev/debug builds, the previously registered handler will be the default
    // handler; in release builds, it will be the one registered by `human_panic`.
    //
    if std::io::stdout().is_terminal() {
        let original_panic_handler = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            // Best-effort attempt to reset the terminal to defaults.
            let _ = try_reset_terminal_to_defaults();

            // Invoke the original handler
            original_panic_handler(panic_info);
        }));
    }
}

/// Run the brush shell. Returns the exit code.
async fn run_async(
    cli_args: Vec<String>,
    args: CommandLineArgs,
    extra_builtins: Vec<(String, brush_core::builtins::Registration)>,
) -> Result<u8, brush_interactive::ShellError> {
    let default_backend = get_default_input_backend();
    let selected_backend = args.input_backend.unwrap_or(default_backend);

    match selected_backend {
        InputBackend::Reedline => {
            run_impl(cli_args, args, shell_factory::ReedlineShellFactory, extra_builtins).await
        }
        InputBackend::Basic => run_impl(cli_args, args, shell_factory::BasicShellFactory, extra_builtins).await,
        InputBackend::Minimal => run_impl(cli_args, args, shell_factory::MinimalShellFactory, extra_builtins).await,
    }
}

async fn run_impl(
    cli_args: Vec<String>,
    args: CommandLineArgs,
    factory: impl shell_factory::ShellFactory + Send + 'static,
    extra_builtins: Vec<(String, brush_core::builtins::Registration)>,
) -> Result<u8, brush_interactive::ShellError> {
    // Initializing tracing.
    // Use the config from entry.rs to share state if needed, or just use it as is.
    let event_config_arc = crate::entry::get_event_config();
    let mut event_config = event_config_arc.try_lock().unwrap();
    *event_config = Some(events::TraceEventConfig::init(
        &args.enabled_debug_events,
        &args.disabled_events,
    ));
    drop(event_config);

    // Instantiate an appropriately configured shell.
    let mut shell = instantiate_shell(&args, cli_args, factory, extra_builtins).await?;

    // Run in that shell.
    let result = run_in_shell(&mut shell, args).await;

    // Display any error that percolated up.
    let exit_code = match result {
        Ok(code) => code,
        Err(brush_interactive::ShellError::ShellError(e)) => {
            let core_shell = shell.shell();
            let mut stderr = core_shell.as_ref().stderr();
            let _ = core_shell.as_ref().display_error(&mut stderr, &e).await;
            1
        }
        Err(err) => {
            tracing::error!("error: {err:#}");
            1
        }
    };

    Ok(exit_code)
}

async fn run_in_shell(
    shell: &mut impl brush_interactive::InteractiveShell,
    args: CommandLineArgs,
) -> Result<u8, brush_interactive::ShellError> {
    // If a command was specified via -c, then run that command and then exit.
    if let Some(command) = args.command {
        // Pass through args.
        if !args.script_args.is_empty() {
            shell.shell_mut().as_mut().shell_name = Some(args.script_args[0].clone());
        }
        shell.shell_mut().as_mut().positional_parameters =
            args.script_args.iter().skip(1).cloned().collect();

        // Execute the command string.
        let params = shell.shell().as_ref().default_exec_params();
        shell
            .shell_mut()
            .as_mut()
            .run_string(command, &params)
            .await?;

    // If -s was provided, then read commands from stdin. If there was a script (and optionally
    // args) passed on the command line via positional arguments, then we copy over the
    // parameters but do *not* execute it.
    } else if args.read_commands_from_stdin {
        if !args.script_args.is_empty() {
            shell
                .shell_mut()
                .as_mut()
                .positional_parameters
                .clone_from(&args.script_args);
        }

        shell.run_interactively().await?;

    // If a script path was provided, then run the script.
    } else if !args.script_args.is_empty() {
        // The path to a script was provided on the command line; run the script.
        shell
            .shell_mut()
            .as_mut()
            .run_script(
                Path::new(&args.script_args[0]),
                args.script_args.iter().skip(1),
            )
            .await?;

    // If we got down here, then we don't have any commands to run. We'll be reading
    // them in from stdin one way or the other.
    } else {
        shell.run_interactively().await?;
    }

    // Make sure to return the last result observed in the shell.
    let result = shell.shell().as_ref().last_result();

    Ok(result)
}

async fn instantiate_shell(
    args: &CommandLineArgs,
    cli_args: Vec<String>,
    factory: impl shell_factory::ShellFactory + Send + 'static,
    extra_builtins: Vec<(String, brush_core::builtins::Registration)>,
) -> Result<impl brush_interactive::InteractiveShell + 'static, brush_interactive::ShellError> {
    let argv0 = if args.sh_mode {
        // Simulate having been run as "sh".
        Some(String::from("sh"))
    } else if !cli_args.is_empty() {
        Some(cli_args[0].clone())
    } else {
        None
    };

    // Commands are read from stdin if -s was provided, or if no command was specified (either via
    // -c or as a positional argument).
    let read_commands_from_stdin = (args.read_commands_from_stdin && args.command.is_none())
        || (args.script_args.is_empty() && args.command.is_none());

    let mut builtins = brush_builtins::default_builtins(if args.sh_mode {
        brush_builtins::BuiltinSet::ShMode
    } else {
        brush_builtins::BuiltinSet::BashMode
    });
    builtins.extend(extra_builtins);

    let fds = args
        .inherited_fds
        .iter()
        .filter_map(|&fd| brush_core::sys::fd::try_get_file_for_open_fd(fd).map(|file| (fd, file)))
        .collect();

    // Compose the options we'll use to create the shell.
    let options = brush_interactive::Options {
        shell: brush_core::CreateOptions {
            disabled_options: args.disabled_options.clone(),
            disabled_shopt_options: args.disabled_shopt_options.clone(),
            disallow_overwriting_regular_files_via_output_redirection: args
                .disallow_overwriting_regular_files_via_output_redirection,
            enabled_options: args.enabled_options.clone(),
            enabled_shopt_options: args.enabled_shopt_options.clone(),
            do_not_execute_commands: args.do_not_execute_commands,
            exit_after_one_command: args.exit_after_one_command,
            login: args.login || argv0.as_ref().is_some_and(|a0| a0.starts_with('-')),
            interactive: args.is_interactive(),
            command_string_mode: args.command.is_some(),
            no_editing: args.no_editing,
            no_profile: args.no_profile,
            no_rc: args.no_rc,
            rc_file: args.rc_file.clone(),
            do_not_inherit_env: args.do_not_inherit_env,
            fds: Some(fds),
            posix: args.posix || args.sh_mode,
            print_commands_and_arguments: args.print_commands_and_arguments,
            read_commands_from_stdin,
            shell_name: argv0,
            shell_product_display_str: Some(productinfo::get_product_display_str()),
            sh_mode: args.sh_mode,
            treat_unset_variables_as_error: args.treat_unset_variables_as_error,
            verbose: args.verbose,
            max_function_call_depth: None,
            key_bindings: None,
            error_formatter: Some(new_error_formatter(args)),
            shell_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            builtins,
        },
        disable_bracketed_paste: args.disable_bracketed_paste,
        disable_color: args.disable_color,
        disable_highlighting: !args.enable_highlighting,
    };

    // Create the shell.
    let mut shell = factory.create(options).await?;

    // Register our own built-in(s) with the shell.
    brushctl::register(shell.shell_mut().as_mut());

    Ok(shell)
}

fn new_error_formatter(
    args: &CommandLineArgs,
) -> Arc<Mutex<dyn brush_core::error::ErrorFormatter>> {
    let formatter = error_formatter::Formatter {
        use_color: !args.disable_color,
    };

    Arc::new(Mutex::new(formatter))
}

fn get_default_input_backend() -> InputBackend {
    #[cfg(any(unix, windows))]
    {
        // If stdin isn't a terminal, then `reedline` doesn't do the right thing
        // (reference: https://github.com/nushell/reedline/issues/509). Switch to
        // the minimal input backend instead for that scenario.
        if std::io::stdin().is_terminal() {
            InputBackend::Reedline
        } else {
            InputBackend::Minimal
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        InputBackend::Minimal
    }
}

fn try_reset_terminal_to_defaults() -> Result<(), std::io::Error> {
    #[cfg(any(unix, windows))]
    {
        // Reset the console.
        let exec_result = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::terminal::EnableLineWrap,
            crossterm::style::ResetColor,
            crossterm::event::DisableMouseCapture,
            crossterm::event::DisableBracketedPaste,
            crossterm::cursor::Show,
            crossterm::cursor::MoveToNextLine(1),
        );

        let raw_result = crossterm::terminal::disable_raw_mode();

        exec_result?;
        raw_result?;
    }

    Ok(())
}
