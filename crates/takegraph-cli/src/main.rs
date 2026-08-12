use clap::{Parser, Subcommand};
use takegraph_node::{VoiceProvider, VoicevoxClient};

#[derive(Debug, Parser)]
#[command(name = "takegraph", version, about = "TakeGraph headless tools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
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
