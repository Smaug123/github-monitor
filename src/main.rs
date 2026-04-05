mod config;
mod facts;
mod github;
mod report;
mod rules;
mod types;
mod workflow;

use crate::github::client::GitHubToken;

fn github_token_from_env() -> Option<GitHubToken> {
    GitHubToken::from_env("GITHUB_TOKEN")
}

fn main() {
    let _github_token = github_token_from_env();
    println!("Hello, world!");
}
