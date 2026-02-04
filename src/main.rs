#![recursion_limit = "256"]

mod ai;
mod config;
mod logging;
mod tui;
mod twitter;
mod widgets;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::{self, Write};

use crate::config::Config;
use crate::tui::App;
use crate::twitter::TwitterClient;

#[derive(Parser)]
#[command(name = "twit")]
#[command(about = "A beautiful TUI client for Twitter/X", long_about = None)]
struct Cli {
    /// Enable debug logging to ~/.cache/twit/twit.log
    #[arg(long, global = true)]
    debug: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Configure Twitter authentication (cookie-based)
    Auth,
    /// Debug: dump raw API response to file
    Debug,
    /// Config utilities
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
    /// Follow a single user
    Follow {
        /// Username to follow (without @)
        username: String,
        /// Don't actually follow, just show what would happen
        #[arg(long)]
        dry_run: bool,
        /// Skip confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Clone another account's following list
    CloneFollows {
        /// Username to clone follows from (without @)
        username: String,
        /// Don't actually follow, just show who would be followed
        #[arg(long)]
        dry_run: bool,
        /// Maximum number of accounts to follow (default: unlimited)
        #[arg(long, short)]
        limit: Option<usize>,
        /// Fast mode for verified accounts (5s delay vs 60s default)
        #[arg(long)]
        fast: bool,
    },
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Validate the current configuration
    Verify,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Enable debug logging if --debug flag or config setting
    if cli.debug {
        logging::enable_debug();
    }

    match cli.command {
        Some(Commands::Auth) => run_auth_wizard().await,
        Some(Commands::Debug) => run_debug().await,
        Some(Commands::Config { command }) => match command {
            ConfigCommands::Verify => run_config_verify(),
        },
        Some(Commands::Follow {
            username,
            dry_run,
            yes,
        }) => run_follow_user(&username, dry_run, yes).await,
        Some(Commands::CloneFollows {
            username,
            dry_run,
            limit,
            fast,
        }) => run_clone_follows(&username, dry_run, limit, fast).await,
        None => run_tui().await,
    }
}

async fn run_auth_wizard() -> Result<()> {
    println!("🐦 Twitter Authentication Setup");
    println!("================================\n");

    println!("To authenticate, you need to get cookies from your browser.\n");
    println!("Steps:");
    println!("  1. Open https://x.com in Chrome/Brave/Firefox");
    println!("  2. Log in to your account");
    println!("  3. Open DevTools (F12 or Cmd+Option+I)");
    println!("  4. Go to Application → Cookies → https://x.com");
    println!("  5. Find and copy the values for:");
    println!("     - auth_token");
    println!("     - ct0\n");

    // Prompt for auth_token
    print!("Enter auth_token: ");
    io::stdout().flush()?;
    let mut auth_token = String::new();
    io::stdin().read_line(&mut auth_token)?;
    let auth_token = auth_token.trim().to_string();

    if auth_token.is_empty() {
        anyhow::bail!("auth_token cannot be empty");
    }

    // Prompt for ct0
    print!("Enter ct0: ");
    io::stdout().flush()?;
    let mut ct0 = String::new();
    io::stdin().read_line(&mut ct0)?;
    let ct0 = ct0.trim().to_string();

    if ct0.is_empty() {
        anyhow::bail!("ct0 cannot be empty");
    }

    println!("\nValidating credentials...");

    // Test the credentials
    let client = TwitterClient::new(auth_token.clone(), ct0.clone())
        .context("Failed to create Twitter client")?;

    match client.get_home_timeline(1).await {
        Ok(tweets) => {
            println!("✓ Authentication successful!");
            if let Some(tweet) = tweets.first() {
                println!("  Found {} tweets in timeline", tweets.len());
                println!("  Latest from: @{}", tweet.user.screen_name);
            }

            // Save config
            let config = Config {
                auth_token,
                ct0,
                ..Config::default()
            };
            config.save().context("Failed to save config")?;

            println!(
                "\n✓ Configuration saved to: {}",
                Config::config_path().display()
            );
            println!("\nYou can now run `twit` to view your timeline!");
        }
        Err(e) => {
            println!("✗ Authentication failed: {:#}", e);
            println!("\nPlease check your cookie values and try again.");
            return Err(e);
        }
    }

    Ok(())
}

async fn run_debug() -> Result<()> {
    let config = Config::load()
        .context("No configuration found. Run `twit auth` to set up authentication.")?;

    if config.debug {
        logging::enable_debug();
    }

    // Warn user about sensitive data
    println!("⚠️  Warning: This command writes raw Twitter API data to disk.");
    println!("   The output file may contain sensitive information such as");
    println!("   user IDs, tweet metadata, and other account details.");
    println!();
    print!("Continue? [y/N] ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    if !input.trim().eq_ignore_ascii_case("y") {
        println!("Aborted.");
        return Ok(());
    }

    let client = TwitterClient::new(config.auth_token, config.ct0)?;

    println!("\nFetching raw timeline response...");
    let raw = client.get_home_timeline_raw(5).await?;

    let output_path = "debug_response.json";
    let pretty = serde_json::to_string_pretty(&raw)?;
    std::fs::write(output_path, &pretty)?;

    println!("Saved raw response to: {}", output_path);
    println!("Response size: {} bytes", pretty.len());

    // Also try parsing and show what we get
    let tweets = client.get_home_timeline(5).await?;
    println!("\nParsed {} tweets:", tweets.len());
    for (i, tweet) in tweets.iter().take(3).enumerate() {
        println!(
            "  {}. @{} ({}): {}",
            i + 1,
            tweet.user.screen_name,
            tweet.user.name,
            tweet.text.chars().take(50).collect::<String>()
        );
    }

    println!();
    println!(
        "💡 Tip: Delete {} when done to avoid leaking data.",
        output_path
    );

    Ok(())
}

async fn run_tui() -> Result<()> {
    // Load config
    let config = Config::load()
        .context("No configuration found. Run `twit auth` to set up authentication.")?;

    // Enable debug logging from config (CLI flag takes precedence, already set in main)
    if config.debug {
        logging::enable_debug();
    }

    // Create and run the app
    let mut app = App::new(config).await?;
    app.run().await
}

fn run_config_verify() -> Result<()> {
    let config = Config::load()
        .context("No configuration found. Run `twit auth` to set up authentication.")?;

    if config.debug {
        logging::enable_debug();
    }

    let report = config.validate();
    println!("Config path: {}", Config::config_path().display());
    println!("AI enabled: {}", config.ai.enabled);

    if report.is_ok() {
        println!("✓ Config looks good");
        if !report.warnings.is_empty() {
            println!("{}", report);
        }
        Ok(())
    } else {
        println!("{}", report);
        anyhow::bail!("Configuration validation failed");
    }
}

async fn run_follow_user(username: &str, dry_run: bool, yes: bool) -> Result<()> {
    let config = Config::load()
        .context("No configuration found. Run `twit auth` to set up authentication.")?;

    if config.debug {
        logging::enable_debug();
    }

    let client = TwitterClient::new(config.auth_token, config.ct0)?;

    let username = username.trim();
    let username = username.trim_start_matches('@');
    if username.is_empty() {
        anyhow::bail!("username cannot be empty");
    }

    println!("Looking up @{}...", username);
    let user_id = client
        .get_user_id_by_screen_name(username)
        .await
        .with_context(|| format!("Failed to find user @{}", username))?;

    println!("Found @{} (id: {}).", username, user_id);

    if dry_run {
        println!("[Dry run] Would follow @{}.", username);
        return Ok(());
    }

    if !yes {
        print!("Proceed with following @{}? [y/N] ", username);
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    client
        .follow_user(&user_id)
        .await
        .with_context(|| format!("Failed to follow @{}", username))?;

    println!("✓ Followed @{}.", username);

    Ok(())
}

async fn run_clone_follows(
    username: &str,
    dry_run: bool,
    limit: Option<usize>,
    fast: bool,
) -> Result<()> {
    let config = Config::load()
        .context("No configuration found. Run `twit auth` to set up authentication.")?;

    if config.debug {
        logging::enable_debug();
    }

    let client = TwitterClient::new(config.auth_token, config.ct0)?;

    // Remove @ if present
    let username = username.trim_start_matches('@');

    println!("Looking up @{}...", username);
    let user_id = client
        .get_user_id_by_screen_name(username)
        .await
        .with_context(|| format!("Failed to find user @{}", username))?;

    println!(
        "Fetching following list for @{} (id: {})...",
        username, user_id
    );

    // Fetch all following with pagination
    let mut all_users = Vec::new();
    let mut cursor: Option<String> = None;
    let mut page = 1;

    loop {
        let page_result = client
            .get_following_page(&user_id, cursor.as_deref())
            .await
            .with_context(|| format!("Failed to fetch following page {}", page))?;

        let count = page_result.users.len();
        all_users.extend(page_result.users);

        println!(
            "  Page {}: {} users (total: {})",
            page,
            count,
            all_users.len()
        );

        // Check if we've hit the limit
        if let Some(max) = limit.filter(|&max| all_users.len() >= max) {
            all_users.truncate(max);
            break;
        }

        match page_result.next_cursor {
            Some(next) if count > 0 => {
                cursor = Some(next);
                page += 1;
                // Small delay to avoid rate limiting
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            }
            _ => break,
        }
    }

    println!("\nFound {} accounts to follow.", all_users.len());

    // Filter out accounts we already follow
    let to_follow: Vec<_> = all_users.iter().filter(|u| !u.following).collect();
    let already_following = all_users.len() - to_follow.len();

    if already_following > 0 {
        println!("Already following {} of them.", already_following);
    }

    if to_follow.is_empty() {
        println!("Nothing new to follow!");
        return Ok(());
    }

    println!("\nAccounts to follow ({}):", to_follow.len());
    for (i, user) in to_follow.iter().enumerate() {
        println!("  {}. @{} ({})", i + 1, user.screen_name, user.name);
    }

    if dry_run {
        println!("\n[Dry run] Would follow {} accounts.", to_follow.len());
        return Ok(());
    }

    // Confirm before proceeding
    println!(
        "\nProceed with following {} accounts? [y/N] ",
        to_follow.len()
    );
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    if !input.trim().eq_ignore_ascii_case("y") {
        println!("Aborted.");
        return Ok(());
    }

    // Follow each account with rate limiting
    // Default: 60s delay (~50/hour to stay under pacing limit)
    // Fast mode (verified accounts): 5s delay
    let base_delay = if fast { 5u64 } else { 60u64 };
    let min_delay = if fast { 3u64 } else { 30u64 };

    let estimated_time = to_follow.len() as u64 * base_delay / 60;
    println!(
        "\nStarting follows ({}s delay, ~{} min estimated)...\n",
        base_delay, estimated_time
    );

    let mut followed = 0;
    let mut failed = 0;
    let mut delay_secs = base_delay;

    for (i, user) in to_follow.iter().enumerate() {
        print!(
            "Following @{} ({}/{})... ",
            user.screen_name,
            i + 1,
            to_follow.len()
        );
        io::stdout().flush()?;

        match client.follow_user(&user.id).await {
            Ok(()) => {
                println!("✓");
                followed += 1;
                // Successful follow - can reduce delay slightly
                delay_secs = delay_secs.saturating_sub(5).max(min_delay);
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("429") || err_str.contains("Rate limit") {
                    println!("✗ (rate limited)");
                    // Duration::try_hours returns None for out-of-range values, but 1 hour is always valid
                    let one_hour = chrono::Duration::try_hours(1)
                        .unwrap_or_else(|| chrono::Duration::minutes(60));
                    let resume_time = chrono::Local::now() + one_hour;
                    println!(
                        "\nRate limited after {} follows ({} failed).",
                        followed, failed
                    );
                    println!(
                        "Try again after {} (in ~1 hour).",
                        resume_time.format("%H:%M")
                    );
                    return Ok(());
                } else {
                    println!("✗ ({})", e);
                    failed += 1;
                }
            }
        }

        // Delay between follows
        if i < to_follow.len() - 1 {
            tokio::time::sleep(tokio::time::Duration::from_secs(delay_secs)).await;
        }
    }

    println!("\nDone! Followed {} accounts, {} failed.", followed, failed);

    Ok(())
}
