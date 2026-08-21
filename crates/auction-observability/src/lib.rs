use thiserror::Error;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Error)]
#[error("failed to initialize tracing subscriber: {message}")]
pub struct InitError {
    message: String,
}

pub fn init(service: &str, configured_filter: Option<&str>) -> Result<(), InitError> {
    let filter = match configured_filter {
        Some(value) => {
            EnvFilter::try_new(value).map_err(|error| InitError { message: error.to_string() })?
        }
        None => EnvFilter::new(format!("info,{service}=debug")),
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .try_init()
        .map_err(|error| InitError { message: error.to_string() })?;

    Ok(())
}
