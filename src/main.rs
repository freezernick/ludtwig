mod analyzer;
mod config;
mod output;
mod process;
mod writer;

use crate::config::Config;
use crate::output::OutputMessage;
use async_std::channel::Sender;
use async_std::path::PathBuf;
use async_std::sync::Arc;
use async_std::{channel, task};
use std::boxed::Box;
use structopt::StructOpt;
use walkdir::{DirEntry, WalkDir};

/// A CLI tool for '.twig' files with focus on formatting and detecting mistakes.
#[derive(StructOpt, Debug, Clone)]
#[structopt(author = env!("CARGO_PKG_AUTHORS"))]
pub struct Opts {
    /// Files or directories to scan and format
    #[structopt(
        value_name = "FILE",
        min_values = 1,
        required = true,
        conflicts_with = "create_config",
        parse(from_os_str),
        name = "files"
    )]
    files: Vec<PathBuf>,

    /// Disable the analysis of the syntax tree. There will still be parsing errors.
    #[structopt(short = "A", long)]
    no_analysis: bool,

    /// Disable the formatted writing of the syntax tree to disk. With this option the tool will not write to any files.
    #[structopt(short = "W", long)]
    no_writing: bool,

    /// Specify a custom output directory instead of modifying the files in place.
    #[structopt(short, long, parse(from_os_str))]
    output_path: Option<PathBuf>,

    /// Specify where the ludtwig configuration file is. Ludtwig looks in the current directory for a 'ludtwig-config.toml' by default.
    #[structopt(short = "c", long, parse(from_os_str))]
    config_path: Option<PathBuf>,

    /// Create the default configuration file in the config path. Defaults to the current directory.
    #[structopt(short = "C", long, name = "create_config")]
    create_config: bool,
}

#[derive(Debug)]
pub struct CliContext {
    /// Channel sender for transmitting messages back to the user.
    pub output_tx: Sender<OutputMessage>,
    /// Disable the analysis of the syntax tree. There will still be parsing errors.
    pub no_analysis: bool,
    /// Disable the formatted writing of the syntax tree to disk. With this option the tool will not write to any files.
    pub no_writing: bool,
    /// Specify a custom output directory instead of modifying the files in place.
    pub output_path: Option<PathBuf>,
    /// The config values to use.
    pub config: Config,
}

impl CliContext {
    /// Helper function to send a [OutputMessage] back to the user.
    pub async fn send_output(&self, msg: OutputMessage) {
        self.output_tx.send(msg).await.unwrap();
    }
}

/// Parse the CLI arguments and bootstrap the async application.
fn main() {
    let opts: Opts = Opts::from_args();
    let config = config::handle_config_or_exit(&opts);

    let process_code = task::block_on(app(opts, config)).unwrap();
    std::process::exit(process_code);
}

/// The entry point of the async application.
async fn app(opts: Opts, config: Config) -> Result<i32, Box<dyn std::error::Error>> {
    println!("Parsing files...");

    // sender and receiver channels for the communication between tasks and the user.
    // the channel is bounded to buffer 32 messages before sending will block (in an async way).
    // this limit should be fine for one task continuously processing the incoming messages from the channel.
    let (tx, rx) = channel::bounded(32);

    let cli_context = Arc::new(CliContext {
        output_tx: tx,
        no_analysis: opts.no_analysis,
        no_writing: opts.no_writing,
        output_path: opts.output_path,
        config,
    });

    let output_handler = task::spawn(output::handle_processing_output(rx));

    let mut futures = Vec::with_capacity(opts.files.len());
    for path in opts.files {
        let context = Arc::clone(&cli_context);
        futures.push(task::spawn(handle_input_path(path, context)));
    }
    drop(cli_context);

    for t in futures {
        t.await;
    }

    // the output_handler will finish execution if all the sending channel ends are closed.
    let process_code = output_handler.await;

    Ok(process_code)
}

/// filters out hidden directories or files
/// (that start with '.').
fn is_hidden(entry: &DirEntry) -> bool {
    !entry
        .file_name()
        .to_str()
        // '.' and './' is a valid path for the current working directory and not an hidden file / dir
        // otherwise anything that starts with a '.' is considered hidden for ludtwig
        .map(|s| s.starts_with('.') && s != "." && s != "./")
        .unwrap_or(false)
}

/// Process a directory path.
async fn handle_input_path(path: PathBuf, cli_context: Arc<CliContext>) {
    let mut futures_processes = Vec::new();
    let walker = WalkDir::new(path).into_iter();

    for entry in walker.filter_entry(|e| is_hidden(e)) {
        let entry = entry.unwrap();

        if !entry.file_type().is_file() {
            continue;
        }

        if !entry
            .file_name()
            .to_str() // also skips non utf-8 file names!
            .map(|s| s.ends_with(".twig"))
            .unwrap_or(false)
        {
            continue;
        }

        futures_processes.push(task::spawn(process::process_file(
            entry.path().into(),
            Arc::clone(&cli_context),
        )));

        // cooperatively give up computation time in this thread to allow other futures to process.
        // because WalkDir is not async this is in fact helpful for the overall performance.
        task::yield_now().await;
    }

    for f in futures_processes {
        f.await;
    }
}
