mod config;
mod facts;
mod github;
mod report;
mod rules;
mod types;
mod workflow;

fn github_token_from_env() -> Option<String> {
    std::env::var("GITHUB_TOKEN").ok()
}

fn main() {
    let _github_token = github_token_from_env();
    println!("Hello, world!");
}
