use clap::Parser;
use color_eyre::Section;
use eyre::{eyre, WrapErr};
use gen-statlib::core::engine::{Engine, EnginePaths};
use gen-statlib::devserver::broker::RenderBehavior;
use gen-statlib::render::highlight::SyntectHighlighter;
use gen-statlib::{AbsPath, RelPath, USER_LOG};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

pub type Result<T> = eyre::Result<T>;

const LOGSPEC: [&str; 3] = [
    "gen-stat_user=info",
    "gen-stat_user=debug",
    "gen-stat=trace,pipeworks=trace,pathmarker=trace,shortcode_processor=trace,typed_path=trace,typed_uri=trace",
];

mod logspec {}

#[derive(clap::Parser)]
#[clap(author, version, about, long_about = None)]
struct Args {
        #[clap(long, default_value = "site-rules.rhai", env = "gen-stat_RULES")]
    rule_script: PathBuf,

        #[clap(long, default_value = "public", env = "gen-stat_OUTPUT")]
    output_dir: PathBuf,

        #[clap(long, default_value = "content", env = "gen-stat_CONTENT")]
    content_dir: PathBuf,

        #[clap(long, default_value = "syntax_themes", env = "gen-stat_SYNTAX_THEMES")]
    syntax_themes_dir: PathBuf,

        #[clap(long, default_value = "web/templates", env = "gen-stat_TEMPLATES")]
    template_dir: PathBuf,

        #[clap(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

        #[clap(short, long)]
    quiet: bool,

    #[clap(subcommand)]
    command: SubCommand,
}

#[derive(Debug, clap::Subcommand)]
enum SubCommand {
        Build(CmdBuild),
        Init(CmdInit),
        Serve(CmdServe),
        BuildSyntaxTheme { path: PathBuf },
}

#[derive(Debug, clap::Args)]
struct CmdBuild {
        #[clap(long)]
    frontmatter: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
struct CmdInit {
        #[clap(default_value = ".")]
    directory: PathBuf,
}

#[derive(Debug, clap::Args)]
struct CmdServe {
        #[clap(long, default_value = "100", env = "gen-stat_DEBOUNCE_MS")]
    debounce_ms: u64,

        #[clap(long, default_value = "127.0.0.1:8000", env = "gen-stat_BIND_ADDR")]
    bind: SocketAddr,
               }

fn install_tracing<S: AsRef<str>>(logspec: S) {
    use tracing_error::ErrorLayer;
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{fmt, EnvFilter};

    let fmt_layer = fmt::layer().with_target(false);
    let filter_layer = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(logspec.as_ref()))
        .unwrap();

    tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt_layer)
        .with(ErrorLayer::default())
        .init();
}

fn main() -> Result<()> {
    use dotenv::dotenv;

    dotenv().ok();

    let args = Args::parse();

         {
        if !args.quiet
            && std::mem::discriminant(&args.command)
                != std::mem::discriminant(&SubCommand::BuildSyntaxTheme { path: "".into() })
        {
            let verbosity = {
                if args.verbose > 2 {
                    2
                } else {
                    args.verbose
                }
            } as usize;
            install_tracing(LOGSPEC[verbosity]);
        }

        let (panic_hook, eyre_hook) = color_eyre::config::HookBuilder::default().into_hooks();

        eyre_hook.install()?;

        std::panic::set_hook(Box::new(move |pi| {
            tracing::error!("{}", panic_hook.panic_report(pi));
        }));
    }

    match &args.command {
        SubCommand::Build(options) => {
            use gen-statlib::core::engine::step::export_frontmatter;

            let paths = engine_paths(&args)?;
            if let Some(path) = &options.frontmatter {
                info!(target: USER_LOG, "exporting frontmatter");

                let engine =
                    Engine::new(Arc::new(paths)).wrap_err("Failed to create new engine")?;

                let target_dir = RelPath::new(path)?;
                let pages = engine.library().iter().map(|(_, page)| page);

                export_frontmatter(&engine, pages, &target_dir)
                    .wrap_err("Failed to export frontmatter")?;
            } else {
                let engine =
                    Engine::new(Arc::new(paths)).wrap_err("Failed to create new engine")?;
                engine.build_site().wrap_err("Failed to build site")?;
            }
        }
        SubCommand::Init(options) => {
            use gen-statlib::init;
            let cwd = std::env::current_dir()?;

            let target = AbsPath::new(cwd.join(&options.directory))?;

            info!(target: USER_LOG, "creating new site at {}", target);

            init::at_target(&target)?;
        }
        SubCommand::Serve(opt) => {
            let paths = engine_paths(&args)?;
            let (handle, _broker) = Engine::with_broker(
                Arc::new(paths),
                opt.bind,
                opt.debounce_ms,
                RenderBehavior::Write,
                             )
            .wrap_err("Failed to initialize engine broker")?;
            let _ = handle.join().map_err(|e| eyre!("{:?}", e))?;
        }
        SubCommand::BuildSyntaxTheme { path } => {
            info!(
                target: USER_LOG,
                "building syntax theme CSS from {}",
                path.display()
            );

            let css_theme = SyntectHighlighter::generate_css_theme(&path).wrap_err_with(|| {
                format!(
                    "Failed to generate CSS output from theme file {}",
                    path.display()
                )
            })?;
            println!("{}", css_theme.css());
        }
    }

    Ok(())
}

fn engine_paths(args: &Args) -> Result<EnginePaths> {
    Ok(EnginePaths {
        rule_script: RelPath::new(&args.rule_script).wrap_err_with(|| {
            format!(
                "Failed to locate rule script at {}",
                &args.rule_script.display()
            )
        })?,
        content_dir: RelPath::new(&args.content_dir).wrap_err_with(|| {
            format!(
                "Failed to locate content dir at {}",
                &args.content_dir.display()
            )
        })?,
        syntax_theme_dir: RelPath::new(&args.syntax_themes_dir).wrap_err_with(|| {
            format!(
                "Failed to locate syntax theme dir at {}",
                &args.syntax_themes_dir.display()
            )
        })?,
        output_dir: RelPath::new(&args.output_dir).wrap_err_with(|| {
            format!(
                "Failed to locate output dir at {}",
                &args.output_dir.display()
            )
        })?,
        template_dir: RelPath::new(&args.template_dir).wrap_err_with(|| {
            format!(
                "Failed to locate template dir at {}",
                &args.template_dir.display()
            )
        })?,
        project_root: AbsPath::new(
            args.rule_script
                .canonicalize()
                .wrap_err_with(||format!("Failed to canonicalize rule script path at {}", &args.rule_script.display()))
                .suggestion(
                    "Make sure 'site-rules.rhai' exists, or set the path manually with --rule-script, or set gen-stat_RULES env",
                )?
                .parent().ok_or_else(|| eyre!("Unable to determine project root from rule script"))?,
        )?,
    })
}
