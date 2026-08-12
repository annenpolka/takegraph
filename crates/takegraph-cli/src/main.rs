use clap::{Parser, Subcommand};
use takegraph_core::{Patch, RevisionId};
use takegraph_node::{VoiceProvider, VoicevoxClient};

#[derive(Debug, Parser)]
#[command(name = "takegraph", version, about = "TakeGraph headless tools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run deterministic patch lifecycle guards for local adapters.
    PatchCommit {
        #[arg(long)]
        base: u64,
        #[arg(long)]
        head: u64,
        #[arg(long)]
        digest: String,
        #[arg(long)]
        approved_digest: String,
    },
    /// Inspect an existing VOICEVOX ENGINE.
    Voicevox {
        #[command(subcommand)]
        command: VoicevoxCommand,
    },
}

#[derive(Debug, Subcommand)]
enum VoicevoxCommand {
    /// Probe manifest, versions, devices, and speakers.
    Probe {
        #[arg(
            long,
            env = "TAKEGRAPH_VOICEVOX_ENDPOINT",
            default_value = "http://127.0.0.1:50021"
        )]
        endpoint: String,
    },
    /// List available speakers and styles.
    Speakers {
        #[arg(
            long,
            env = "TAKEGRAPH_VOICEVOX_ENDPOINT",
            default_value = "http://127.0.0.1:50021"
        )]
        endpoint: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Command::PatchCommit {
            base,
            head,
            digest,
            approved_digest,
        } => {
            if approved_digest != digest {
                return Err("approval does not match the staged patch digest".into());
            }

            let mut patch = Patch::draft(RevisionId(base), digest);
            patch.validate()?;
            patch.materialize_preview()?;
            patch.approve()?;
            let revision = patch.commit(RevisionId(head))?;
            println!("{}", serde_json::json!({ "revision": revision.0 }));
        }
        Command::Voicevox { command } => match command {
            VoicevoxCommand::Probe { endpoint } => {
                let capabilities = VoicevoxClient::new(&endpoint)?.probe().await?;
                println!("{}", serde_json::to_string_pretty(&capabilities)?);
            }
            VoicevoxCommand::Speakers { endpoint } => {
                let capabilities = VoicevoxClient::new(&endpoint)?.probe().await?;
                println!("{}", serde_json::to_string_pretty(&capabilities.speakers)?);
            }
        },
    }

    Ok(())
}
