pub mod assets;
pub mod client;
pub mod config;
pub mod duplicates;
pub mod health;
pub mod instance_info;
pub mod oauth;
pub mod oauth_resources;
pub mod post;
pub mod search;
pub mod server;
pub mod settings;
pub mod single_flight;
pub mod subreddit;
pub mod user;
pub mod utils;

#[cfg(test)]
mod css_checks;
#[cfg(test)]
mod html_checks;
#[cfg(test)]
mod test_support;
